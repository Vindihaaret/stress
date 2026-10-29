//! GPU load: a headless OpenGL ES context on one chosen GPU through the EGL
//! device platform, drawing a heavy fragment shader for `load` percent of every
//! 50 ms. Each GPU is loaded by its own worker process (`stresstest
//! gpu-worker <pci>`), so a driver crash or hang cannot take the app with it.
//!
//! Single draws are kept to a few milliseconds, so the desktop stays responsive
//! even when the loaded GPU is also the one driving the screen.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::dl::{self, Lib};

const PERIOD: Duration = Duration::from_millis(50);
const WIDTH: i32 = 2048;
const HEIGHT: i32 = 2048;
/// Target length of a single draw.
const DRAW_TIME: Duration = Duration::from_millis(4);

// ---------------------------------------------------------------- parent side

pub struct GpuLoad {
    child: Child,
    stdin: ChildStdin,
    /// GL_RENDERER as reported by the driver, to confirm the right GPU is used.
    pub renderer: String,
}

impl GpuLoad {
    pub fn start(pci: &str, load: u32) -> Result<GpuLoad, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(exe)
            .args(["gpu-worker", pci])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start the GPU worker: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        // The first line says whether the GPU could be opened.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            let _ = tx.send(lines.next().and_then(Result::ok));
            for _ in lines {}
        });
        let reply = rx.recv_timeout(Duration::from_secs(15));
        let fail = |mut child: Child, msg: String| {
            let _ = child.kill();
            let _ = child.wait();
            Err(msg)
        };
        let renderer = match reply {
            Ok(Some(line)) => match line.split_once(' ') {
                Some(("ready", renderer)) => renderer.to_string(),
                Some(("error", msg)) => return fail(child, msg.to_string()),
                _ => return fail(child, format!("unexpected reply from the GPU worker: {line}")),
            },
            Ok(None) => return fail(child, "the GPU worker stopped while opening the GPU".into()),
            Err(_) => return fail(child, "the GPU did not respond within 15 seconds".into()),
        };
        let mut gpu = GpuLoad { child, stdin, renderer };
        gpu.set(load);
        Ok(gpu)
    }

    pub fn set(&mut self, load: u32) {
        let _ = writeln!(self.stdin, "load {}", load.min(100));
    }

    /// False when the worker has exited (crash or driver error).
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn stop(self) {}
}

impl Drop for GpuLoad {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// PCI addresses of the GPUs EGL can open, without creating any context.
pub fn loadable() -> Vec<String> {
    let Ok(egl) = Egl::load() else { return Vec::new() };
    egl.devices().into_iter().filter_map(|(_, pci)| pci).collect()
}

// ---------------------------------------------------------------- worker side

/// Entry point of `stresstest gpu-worker <pci>`. Protocol on stdin: `load N`
/// and `quit`; end of input (the app is gone) also stops.
pub fn worker_main(pci: &str) -> i32 {
    unsafe {
        let parent = libc::getppid();
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
        if libc::getppid() != parent {
            return 1;
        }
    }
    let gl = match Context::open(pci) {
        Ok(gl) => gl,
        Err(e) => {
            println!("error {e}");
            return 1;
        }
    };
    println!("ready {}", gl.renderer);
    let _ = std::io::stdout().flush();

    let load = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (load, stop) = (load.clone(), stop.clone());
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                match line.split_once(' ') {
                    Some(("load", v)) => load.store(v.trim().parse().unwrap_or(0), Ordering::Relaxed),
                    _ if line.trim() == "quit" => break,
                    _ => {}
                }
            }
            stop.store(true, Ordering::Relaxed);
        });
    }

    let mut iterations = gl.calibrate();
    let mut seed = 0.0f32;
    while !stop.load(Ordering::Relaxed) {
        let start = Instant::now();
        let percent = load.load(Ordering::Relaxed).min(100);
        if percent == 0 {
            std::thread::sleep(PERIOD);
            continue;
        }
        let busy = PERIOD * percent / 100;
        while start.elapsed() < busy {
            let t = Instant::now();
            seed = (seed + 0.013) % 1.0;
            gl.draw(iterations, seed);
            // Keep draws short also when clocks change under load.
            let took = t.elapsed();
            if took > DRAW_TIME * 2 && iterations > 1 {
                iterations /= 2;
            } else if took < DRAW_TIME / 2 && iterations < 1 << 16 {
                iterations *= 2;
            }
        }
        if percent < 100 {
            std::thread::sleep(PERIOD.saturating_sub(start.elapsed()));
        }
    }
    0
}

// ---------------------------------------------------------------- EGL

type Display = *mut c_void;
type Handle = *mut c_void;

const EGL_NONE: i32 = 0x3038;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_PBUFFER_BIT: i32 = 0x0001;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES3_BIT: i32 = 0x0040;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_CONTEXT_MAJOR_VERSION: i32 = 0x3098;
const EGL_PLATFORM_DEVICE_EXT: u32 = 0x313F;
const EGL_DRM_DEVICE_FILE_EXT: i32 = 0x3233;

struct Egl {
    get_proc: unsafe extern "C" fn(*const c_char) -> *const c_void,
    initialize: unsafe extern "C" fn(Display, *mut i32, *mut i32) -> u32,
    bind_api: unsafe extern "C" fn(u32) -> u32,
    choose_config: unsafe extern "C" fn(Display, *const i32, *mut Handle, i32, *mut i32) -> u32,
    create_pbuffer: unsafe extern "C" fn(Display, Handle, *const i32) -> Handle,
    create_context: unsafe extern "C" fn(Display, Handle, Handle, *const i32) -> Handle,
    make_current: unsafe extern "C" fn(Display, Handle, Handle, Handle) -> u32,
    get_error: unsafe extern "C" fn() -> i32,
    query_devices: unsafe extern "C" fn(i32, *mut Handle, *mut i32) -> u32,
    query_device_string: unsafe extern "C" fn(Handle, i32) -> *const c_char,
    platform_display: unsafe extern "C" fn(u32, Handle, *const i32) -> Display,
}

impl Egl {
    fn load() -> Result<Egl, String> {
        let lib = Lib::open("libEGL.so.1").ok_or("libEGL.so.1 is not installed")?;
        let get_proc: unsafe extern "C" fn(*const c_char) -> *const c_void =
            unsafe { lib.sym("eglGetProcAddress") }.ok_or("libEGL has no eglGetProcAddress")?;
        // Core functions through dlsym, extensions through eglGetProcAddress.
        let f = |name: &str| -> Result<*const c_void, String> {
            lib.raw(name).or_else(|| proc_address(get_proc, name)).ok_or_else(|| format!("EGL lacks {name}"))
        };
        unsafe {
            Ok(Egl {
                get_proc,
                initialize: dl::cast(f("eglInitialize")?),
                bind_api: dl::cast(f("eglBindAPI")?),
                choose_config: dl::cast(f("eglChooseConfig")?),
                create_pbuffer: dl::cast(f("eglCreatePbufferSurface")?),
                create_context: dl::cast(f("eglCreateContext")?),
                make_current: dl::cast(f("eglMakeCurrent")?),
                get_error: dl::cast(f("eglGetError")?),
                query_devices: dl::cast(f("eglQueryDevicesEXT")?),
                query_device_string: dl::cast(f("eglQueryDeviceStringEXT")?),
                platform_display: dl::cast(f("eglGetPlatformDisplayEXT")?),
            })
        }
    }

    /// Every EGL device with the PCI address of its DRM node, when it has one.
    /// Software renderers have no DRM node and are left out.
    fn devices(&self) -> Vec<(Handle, Option<String>)> {
        let mut handles = [std::ptr::null_mut(); 16];
        let mut count = 0;
        if unsafe { (self.query_devices)(handles.len() as i32, handles.as_mut_ptr(), &mut count) } == 0 {
            return Vec::new();
        }
        handles[..count.max(0) as usize]
            .iter()
            .map(|&dev| {
                let file = unsafe { (self.query_device_string)(dev, EGL_DRM_DEVICE_FILE_EXT) };
                let pci = (!file.is_null())
                    .then(|| unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned())
                    .and_then(|file| drm_pci_address(&file));
                (dev, pci)
            })
            .filter(|(_, pci)| pci.is_some())
            .collect()
    }

    fn error(&self, what: &str) -> String {
        format!("{what} failed (EGL error 0x{:x})", unsafe { (self.get_error)() })
    }
}

fn proc_address(get_proc: unsafe extern "C" fn(*const c_char) -> *const c_void, name: &str) -> Option<*const c_void> {
    let name = CString::new(name).ok()?;
    let ptr = unsafe { get_proc(name.as_ptr()) };
    (!ptr.is_null()).then_some(ptr)
}

/// `/dev/dri/card1` -> `0000:01:00.0`.
fn drm_pci_address(file: &str) -> Option<String> {
    let node = Path::new(file).file_name()?;
    let device = std::fs::canonicalize(Path::new("/sys/class/drm").join(node).join("device")).ok()?;
    Some(device.file_name()?.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- GL

const GL_VERTEX_SHADER: c_uint = 0x8B31;
const GL_FRAGMENT_SHADER: c_uint = 0x8B30;
const GL_COMPILE_STATUS: c_uint = 0x8B81;
const GL_LINK_STATUS: c_uint = 0x8B82;
const GL_TRIANGLES: c_uint = 0x0004;
const GL_RENDERER: c_uint = 0x1F01;

const VERTEX_SHADER: &str = "#version 300 es
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

// Transcendental and multiply-add work per pixel, with a result that depends
// on every step so the compiler cannot drop any of it.
const FRAGMENT_SHADER: &str = "#version 300 es
precision highp float;
uniform int iterations;
uniform float seed;
out vec4 color;
void main() {
    vec4 a = vec4(gl_FragCoord.xy * 0.0007, gl_FragCoord.yx * 0.0011) + seed;
    vec4 b = a.yzwx + 0.1, c = a.zwxy + 0.2, d = a.wxyz + 0.3;
    for (int i = 0; i < iterations; i++) {
        a = a * 0.9999 + b * 0.0001; b = b * 0.9998 + c * 0.0002;
        c = c * 0.9997 + d * 0.0003; d = d * 0.9996 + a * 0.0004;
        a = a * 0.9995 + c * 0.0005; b = b * 0.9994 + d * 0.0006;
        c = c * 0.9993 + a * 0.0007; d = d * 0.9992 + b * 0.0008;
        a = sin(a);
    }
    color = a + b + c + d;
}
";

struct Context {
    renderer: String,
    iterations: c_int,
    seed: c_int,
    uniform1i: unsafe extern "C" fn(c_int, c_int),
    uniform1f: unsafe extern "C" fn(c_int, f32),
    draw_arrays: unsafe extern "C" fn(c_uint, c_int, c_int),
    finish: unsafe extern "C" fn(),
}

impl Context {
    fn open(pci: &str) -> Result<Context, String> {
        let egl = Egl::load()?;
        let device = egl
            .devices()
            .into_iter()
            .find(|(_, p)| p.as_deref() == Some(pci))
            .map(|(d, _)| d)
            .ok_or_else(|| format!("EGL cannot use the GPU at {pci}"))?;
        unsafe {
            let display = (egl.platform_display)(EGL_PLATFORM_DEVICE_EXT, device, [EGL_NONE].as_ptr());
            if display.is_null() {
                return Err(egl.error("eglGetPlatformDisplayEXT"));
            }
            let (mut major, mut minor) = (0, 0);
            if (egl.initialize)(display, &mut major, &mut minor) == 0 {
                return Err(egl.error("eglInitialize"));
            }
            if (egl.bind_api)(EGL_OPENGL_ES_API) == 0 {
                return Err(egl.error("eglBindAPI"));
            }
            let attribs = [
                EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
                EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE,
            ];
            let mut config = std::ptr::null_mut();
            let mut found = 0;
            if (egl.choose_config)(display, attribs.as_ptr(), &mut config, 1, &mut found) == 0 || found < 1 {
                return Err("the GPU offers no OpenGL ES 3 configuration".into());
            }
            let surface = (egl.create_pbuffer)(display, config, [EGL_WIDTH, WIDTH, EGL_HEIGHT, HEIGHT, EGL_NONE].as_ptr());
            if surface.is_null() {
                return Err(egl.error("eglCreatePbufferSurface"));
            }
            let context = (egl.create_context)(display, config, std::ptr::null_mut(), [EGL_CONTEXT_MAJOR_VERSION, 3, EGL_NONE].as_ptr());
            if context.is_null() {
                return Err(egl.error("eglCreateContext"));
            }
            if (egl.make_current)(display, surface, surface, context) == 0 {
                return Err(egl.error("eglMakeCurrent"));
            }
            Context::build(&egl)
        }
    }

    unsafe fn build(egl: &Egl) -> Result<Context, String> {
        let f = |name: &str| proc_address(egl.get_proc, name).ok_or_else(|| format!("OpenGL ES lacks {name}"));
        unsafe {
            let get_string: unsafe extern "C" fn(c_uint) -> *const c_char = dl::cast(f("glGetString")?);
            let create_shader: unsafe extern "C" fn(c_uint) -> c_uint = dl::cast(f("glCreateShader")?);
            let shader_source: unsafe extern "C" fn(c_uint, c_int, *const *const c_char, *const c_int) =
                dl::cast(f("glShaderSource")?);
            let compile_shader: unsafe extern "C" fn(c_uint) = dl::cast(f("glCompileShader")?);
            let get_shader_iv: unsafe extern "C" fn(c_uint, c_uint, *mut c_int) = dl::cast(f("glGetShaderiv")?);
            let shader_log: unsafe extern "C" fn(c_uint, c_int, *mut c_int, *mut c_char) =
                dl::cast(f("glGetShaderInfoLog")?);
            let create_program: unsafe extern "C" fn() -> c_uint = dl::cast(f("glCreateProgram")?);
            let attach_shader: unsafe extern "C" fn(c_uint, c_uint) = dl::cast(f("glAttachShader")?);
            let link_program: unsafe extern "C" fn(c_uint) = dl::cast(f("glLinkProgram")?);
            let get_program_iv: unsafe extern "C" fn(c_uint, c_uint, *mut c_int) = dl::cast(f("glGetProgramiv")?);
            let use_program: unsafe extern "C" fn(c_uint) = dl::cast(f("glUseProgram")?);
            let uniform_location: unsafe extern "C" fn(c_uint, *const c_char) -> c_int =
                dl::cast(f("glGetUniformLocation")?);
            let viewport: unsafe extern "C" fn(c_int, c_int, c_int, c_int) = dl::cast(f("glViewport")?);
            let gen_vertex_arrays: unsafe extern "C" fn(c_int, *mut c_uint) = dl::cast(f("glGenVertexArrays")?);
            let bind_vertex_array: unsafe extern "C" fn(c_uint) = dl::cast(f("glBindVertexArray")?);

            let renderer = get_string(GL_RENDERER);
            let renderer = if renderer.is_null() {
                "unknown".to_string()
            } else {
                CStr::from_ptr(renderer).to_string_lossy().into_owned()
            };

            let compile = |kind: c_uint, source: &str| -> Result<c_uint, String> {
                let shader = create_shader(kind);
                let source = CString::new(source).expect("shader source");
                shader_source(shader, 1, &source.as_ptr(), std::ptr::null());
                compile_shader(shader);
                let mut ok = 0;
                get_shader_iv(shader, GL_COMPILE_STATUS, &mut ok);
                if ok == 0 {
                    let mut log = [0 as c_char; 1024];
                    shader_log(shader, log.len() as c_int, std::ptr::null_mut(), log.as_mut_ptr());
                    return Err(format!("shader error: {}", CStr::from_ptr(log.as_ptr()).to_string_lossy()));
                }
                Ok(shader)
            };
            let program = create_program();
            attach_shader(program, compile(GL_VERTEX_SHADER, VERTEX_SHADER)?);
            attach_shader(program, compile(GL_FRAGMENT_SHADER, FRAGMENT_SHADER)?);
            link_program(program);
            let mut ok = 0;
            get_program_iv(program, GL_LINK_STATUS, &mut ok);
            if ok == 0 {
                return Err("the load shader could not be linked".into());
            }
            use_program(program);
            let mut vao = 0;
            gen_vertex_arrays(1, &mut vao);
            bind_vertex_array(vao);
            viewport(0, 0, WIDTH, HEIGHT);

            Ok(Context {
                renderer,
                iterations: uniform_location(program, c"iterations".as_ptr()),
                seed: uniform_location(program, c"seed".as_ptr()),
                uniform1i: dl::cast(f("glUniform1i")?),
                uniform1f: dl::cast(f("glUniform1f")?),
                draw_arrays: dl::cast(f("glDrawArrays")?),
                finish: dl::cast(f("glFinish")?),
            })
        }
    }

    fn draw(&self, iterations: c_int, seed: f32) {
        unsafe {
            (self.uniform1i)(self.iterations, iterations);
            (self.uniform1f)(self.seed, seed);
            (self.draw_arrays)(GL_TRIANGLES, 0, 3);
            (self.finish)();
        }
    }

    /// Finds the shader length that makes one draw take about [`DRAW_TIME`].
    fn calibrate(&self) -> c_int {
        let mut iterations = 4;
        for _ in 0..20 {
            let t = Instant::now();
            self.draw(iterations, 0.5);
            let took = t.elapsed();
            if took >= DRAW_TIME / 2 || iterations >= 1 << 16 {
                break;
            }
            iterations *= 2;
        }
        iterations
    }
}
