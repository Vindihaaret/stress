#!/bin/sh
# Removes what setup-rapl.sh installed and makes the processor's energy
# counters readable by root only again, as the kernel has them by default.
# Must run as root; Stress runs it through pkexec.
set -eu

rm -f /etc/udev/rules.d/60-stresstest-rapl.rules
udevadm control --reload-rules 2>/dev/null || true
for counter in /sys/class/powercap/intel-rapl:*/energy_uj; do
    [ -e "$counter" ] && chmod go-r "$counter"
done
echo "Processor power access is removed."
