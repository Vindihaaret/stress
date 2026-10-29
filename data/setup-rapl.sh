#!/bin/sh
# Lets programs read the processor's energy counters (RAPL), so Stress can
# show the processor's power use. Must run as root; Stress runs it through
# pkexec. Undone by remove-rapl.sh.
set -eu

RULE=/etc/udev/rules.d/60-stresstest-rapl.rules

cat > "$RULE" <<'RULES'
# Installed by Stress: makes the processor's energy counters readable, so
# programs can show its power use. Delete this file to undo.
ACTION=="add", SUBSYSTEM=="powercap", KERNEL=="intel-rapl:*", RUN+="/bin/chmod a+r /sys%p/energy_uj"
RULES
udevadm control --reload-rules 2>/dev/null || true

# Apply it now too, without waiting for a restart.
for counter in /sys/class/powercap/intel-rapl:*/energy_uj; do
    [ -e "$counter" ] && chmod a+r "$counter"
done
echo "Processor power access is set up."
