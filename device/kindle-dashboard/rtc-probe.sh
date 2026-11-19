#!/bin/sh
set -u

mode=$1
root=$2
session=$3
delay=45

log() {
    printf '%s [rtc] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >&2
}

fail() {
    log "ERROR $*"
    exit 1
}

on_battery() {
    [ "$(cat "$online" 2>/dev/null)" = 0 ]
}

epoch_valid() {
    case "${1:-}" in ''|*[!0-9]*) return 1 ;; esac
}

read_matching_clock() {
    now=$(date +%s) && rtc_now=$(cat "$rtc_dir/since_epoch") || return 1
    epoch_valid "$now" && epoch_valid "$rtc_now" || return 1
    difference=$((now - rtc_now))
    [ "$difference" -ge -5 ] && [ "$difference" -le 5 ]
}

rtc=$(cat "$root/rtc-device" 2>/dev/null) ||
    fail "Choose the verified rtc0 or rtc1 in rtc-device after the panel smoke test"
case "$rtc" in rtc0|rtc1) ;; *) fail "Unexpected RTC device $rtc" ;; esac
rtc_dir="/sys/class/rtc/$rtc"
alarm="$rtc_dir/wakealarm"

online=$(cat "$root/external-power-path" 2>/dev/null) ||
    fail "Set external-power-path to the device's verified power-supply online attribute"
case "$online" in /sys/class/power_supply/*/online) ;; *) fail "Invalid power attribute" ;; esac
case "$online" in *..*|*[!A-Za-z0-9_/.-]*) fail "Invalid power attribute" ;; esac
on_battery || fail "External power is present or its status is unknown"
grep -qw mem /sys/power/state && [ -w /sys/power/state ] ||
    fail "Kernel does not offer writable mem suspend"
if [ -r /sys/power/mem_sleep ]; then
    mem_mode=$(cat /sys/power/mem_sleep) || fail "Cannot read mem suspend mode"
    case "$mem_mode" in *'[deep]'*) ;; *) fail "Deep suspend is not selected: $mem_mode" ;; esac
fi
[ -w "$alarm" ] || fail "$alarm is not writable"
read_matching_clock || fail "RTC and system time differ or could not be read"
active=$(cat "$alarm") || fail "Cannot read current alarm"
case "$active" in ''|0) ;; *) fail "RTC already has an alarm ($active); leaving it untouched" ;; esac

[ "$mode" != check ] || exit 0
[ "$mode" = run ] || fail "Unknown probe mode"

target=$((rtc_now + delay))
printf '%s %s\n' "$rtc" "$target" > "$session/alarm" || fail "Could not journal the alarm"
# The supervisor clears this target on all exit paths if it still owns the alarm.
printf '%s\n' "$target" > "$alarm" || fail "Failed to arm RTC"
[ "$(cat "$alarm")" = "$target" ] || fail "RTC alarm readback does not match"
log "Armed $rtc for $target. Press the power button if the probe does not wake."
on_battery || fail "External power changed before suspend"
[ "$(cat "$alarm")" = "$target" ] || fail "RTC alarm changed before suspend"

log "Calling mem suspend at $(date +%s)"
printf 'mem\n' > /sys/power/state || fail "Kernel rejected suspend"
resumed=$(cat "$rtc_dir/since_epoch") || fail "Could not read RTC on return"
epoch_valid "$resumed" || fail "Invalid RTC timestamp on return"
delta=$((resumed - target))
log "Kernel returned at $resumed (target $target, delta ${delta}s)"
if [ "$delta" -lt 0 ]; then
    log "EARLY RETURN: hardware wake or suspend did not hold until the alarm"
    exit 2
fi
[ "$delta" -le 10 ] || fail "Returned too late to count as a likely RTC wake"
log "RTC CANDIDATE PASS: return near the alarm. Confirm with the kernel suspend log or measured behavior."
