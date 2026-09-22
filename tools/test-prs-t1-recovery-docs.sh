#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
agent_readme="$repo_root/crates/prs-t1-agent/README.md"
build_guide="$repo_root/crates/prs-t1-agent/build.md"

grep -Fq '## USB wake/recovery while native UI owns the reader' "$agent_readme"
grep -Fq 'adb shell input keyevent 116' "$agent_readme"
grep -Fq 'KEY_POWER` (type `1`, code `116`)' "$agent_readme"
grep -Fq 'adb shell /data/local/tmp/prs-t1-agent input' "$agent_readme"
grep -Fq '/dev/input/event2' "$agent_readme"
grep -Fq 'wm831x_on' "$agent_readme"
grep -Fq '/dev/input/event4' "$agent_readme"
grep -Fq 'sub_cpu_pwrbutton' "$agent_readme"
grep -Fq 'type_name=KEY code=116 code_name=KEY_POWER' "$agent_readme"
grep -Fq 'sendevent /dev/input/event2 1 116 1' "$agent_readme"
grep -Fq 'sendevent /dev/input/event2 0 0 0' "$agent_readme"
grep -Fq 'sendevent /dev/input/event2 1 116 0' "$agent_readme"
grep -Fq '/data/local/tmp/prs-t1-power-state on' "$agent_readme"
grep -Fq 'physical power button or the hardware reset procedure' "$agent_readme"
grep -Fq 'development/recovery technique only' "$agent_readme"
grep -Fq '[power-state helper build instructions](build.md#power-state-helper)' "$agent_readme"

grep -Fq '## Power-state helper' "$build_guide"
grep -Fq 'adb pull /system/lib/libdl.so /tmp/prs-t1-libdl.so' "$build_guide"
grep -Fq 'build-power-state-helper.sh' "$build_guide"
grep -Fq "reader's own \`/system/lib/libdl.so\`" "$build_guide"

printf '%s\n' 'PRS-T1 USB wake/recovery documentation checks passed'
