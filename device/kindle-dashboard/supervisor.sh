#!/bin/sh
set -u

root=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd) || exit 1
mode=${1:-}
panel_mode=${2:-panel}
powerd=com.lab126.powerd

log() {
    printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >&2
}

fail() {
    log "ERROR $*"
    exit 1
}

# Linux start time distinguishes our process from a reused PID.
process_id() {
    [ -n "$1" ] && [ -r "/proc/$1/stat" ] || return 1
    awk '{ sub(/^.*\) /, ""); if ($1 != "Z") print $20 }' "/proc/$1/stat"
}

owned_process() {
    case "${1:-}:${2:-}" in
        :*|*:|*[!0-9:]*) return 1 ;;
    esac
    [ "$1" -gt 1 ] && [ "$(process_id "$1")" = "$2" ]
}

stop_child() {
    [ -r "$session/child" ] || return 0
    read -r child child_id < "$session/child" || return 1
    if owned_process "$child" "$child_id"; then
        kill -TERM "$child" 2>/dev/null || :
        for attempt in 1 2 3; do
            owned_process "$child" "$child_id" || break
            sleep 1
        done
        if owned_process "$child" "$child_id"; then
            kill -KILL "$child" 2>/dev/null || :
            sleep 1
        fi
    fi
    ! owned_process "$child" "$child_id"
}

ram_directory() {
    for candidate in /var/tmp /dev/shm; do
        dir=$(readlink -f "$candidate" 2>/dev/null) || continue
        [ -d "$dir" ] && [ -w "$dir" ] || continue
        awk -v dir="$dir" '
            ($2 == "/" || dir == $2 || index(dir, $2 "/") == 1) && length($2) > length(mount) {
                mount = $2; type = $3; options = $4
            }
            END { exit !(type == "tmpfs" && options !~ /(^|,)noexec(,|$)/) }
        ' /proc/mounts || continue
        printf '%s\n' "$dir"
        return
    done
    return 1
}

job_running() {
    case "$(initctl status "$1" 2>/dev/null)" in
        "$1 start/running"*) return 0 ;;
        *) return 1 ;;
    esac
}

library_ready() {
    job_running framework && job_running pillow || return 1
    [ "$(lipc-get-prop com.lab126.appmgrd activeApp 2>/dev/null)" = com.lab126.KPPMainApp ] || return 1
    windows=$(DISPLAY=:0 xwininfo -root -tree 2>/dev/null) || return 1
    printf '%s\n' "$windows" | grep -q 'N:application.*KPPMainApp.*MapState=IsViewable' || return 1
    ! printf '%s\n' "$windows" | grep -q 'PILLOW:simple-alert.*MapState=IsViewable'
}

pause_process() {
    pid=$1
    journal=$2
    identity=$(process_id "$pid")
    owned_process "$pid" "$identity" || return 1
    case "$(awk '{ sub(/^.*\) /, ""); print $1 }' "/proc/$pid/stat")" in
        T|t|'') return 1 ;;
    esac
    printf '%s %s\n' "$pid" "$identity" > "$journal" || return 1
    kill -STOP "$pid"
}

resume_process() {
    journal=$1
    [ -f "$journal" ] || return 0
    read -r pid identity < "$journal" || return 1
    if owned_process "$pid" "$identity" && ! kill -CONT "$pid"; then
        return 1
    fi
    rm -f "$journal"
}

restore_window_manager() {
    [ -f "$session/awesome" ] || return 0
    # Restore the native frame before letting the window manager draw again.
    if [ -f "$session/framebuffer" ]; then
        cat "$session/framebuffer" > /dev/fb0 || log "Could not restore saved framebuffer; requesting repaint"
    fi
    resume_process "$session/awesome"
}

restore_rtc() {
    [ -f "$session/alarm" ] || return 0
    read -r rtc epoch < "$session/alarm" || return 1
    case "$rtc" in rtc0|rtc1) ;; *) return 1 ;; esac
    case "$epoch" in ''|*[!0-9]*) return 1 ;; esac
    alarm="/sys/class/rtc/$rtc/wakealarm"
    active=$(cat "$alarm") || return 1
    if [ "$active" = "$epoch" ]; then
        printf '0\n' > "$alarm" || return 1
        log "Cleared owned RTC alarm $epoch"
    fi
    rm -f "$session/alarm"
}

restore_frontlight() {
    [ -f "$session/previous-frontlight" ] || return 0
    level=$(cat "$session/previous-frontlight") || return 1
    case "$level" in ''|*[!0-9]*) return 1 ;; esac
    timeout 2 lipc-set-prop -i "$powerd" flIntensity "$level" &&
        [ "$(timeout 2 lipc-get-prop "$powerd" flIntensity)" = "$level" ] || return 1
    rm -f "$session/previous-frontlight"
    log "Restored front light=$level"
}

restore_wifi() {
    [ -f "$session/previous-wifi" ] || return 0
    read -r wireless radio < "$session/previous-wifi" || return 1
    case "$wireless:$radio" in 0:0|0:1|0:-1|1:0|1:1|1:-1) ;; *) return 1 ;; esac
    if [ "$wireless" = 1 ]; then
        timeout 2 lipc-set-prop -i com.lab126.cmd wirelessEnable "$wireless" || return 1
    fi
    if [ "$radio" != -1 ] && [ "$(timeout 2 lipc-get-prop com.lab126.wifid enable)" != "$radio" ]; then
        timeout 2 lipc-set-prop -i com.lab126.wifid enable "$radio" || return 1
    fi
    if [ "$wireless" = 0 ]; then
        timeout 2 lipc-set-prop -i com.lab126.cmd wirelessEnable "$wireless" || return 1
    fi
    [ "$(timeout 2 lipc-get-prop com.lab126.cmd wirelessEnable)" = "$wireless" ] || return 1
    rm -f "$session/previous-wifi"
    log "Restored Wi-Fi=$wireless"
}

restore_session() {
    ok=1
    if ! stop_child; then
        log "Could not stop the dashboard child"
        return 1
    fi
    if ! restore_rtc; then
        log "Could not restore RTC"
        ok=0
    fi
    if ! restore_frontlight; then
        log "Could not restore native front light"
        ok=0
    fi
    if ! restore_wifi; then
        log "Could not restore native Wi-Fi"
        ok=0
    fi
    # Recover journals written by the earlier full restart launcher as well.
    if [ -f "$session/gui" ]; then
        gui=$(cat "$session/gui")
        case "$gui" in framework|lab126_gui) ;; *) return 1 ;; esac
        job_running "$gui" || initctl start "$gui" || :
        for attempt in 1 2 3 4 5; do
            job_running "$gui" && break
            sleep 1
        done
        if job_running "$gui"; then
            log "Native job is running: $gui"
        else
            log "Could not restore native job: $gui"
            ok=0
        fi
    fi
    if ! restore_window_manager; then
        log "Could not resume the window manager"
        ok=0
    fi
    if ! resume_process "$session/statusbar"; then
        log "Could not resume the native status bar"
        ok=0
    fi
    if [ -f "$session/pillow" ]; then
        if lipc-set-prop com.lab126.pillow disableEnablePillow enable; then
            rm -f "$session/pillow"
        else
            log "Could not re-enable native dialogs"
            ok=0
        fi
    fi
    if [ -f "$session/previous-screen-saver" ]; then
        previous=$(cat "$session/previous-screen-saver")
        case "$previous" in
            0|1)
                if lipc-set-prop -i "$powerd" preventScreenSaver "$previous" &&
                    [ "$(lipc-get-prop "$powerd" preventScreenSaver)" = "$previous" ]; then
                    rm -f "$session/previous-screen-saver"
                    log "Restored preventScreenSaver=$previous"
                else
                    log "Could not restore preventScreenSaver=$previous"
                    ok=0
                fi
                ;;
            *) ok=0 ;;
        esac
    fi
    [ "$ok" = 1 ] || return 1
    if [ -f "$session/gui" ] || [ -f "$session/handoff" ]; then
        if [ "$(lipc-get-prop com.lab126.appmgrd activeApp 2>/dev/null)" != com.lab126.KPPMainApp ]; then
            lipc-set-prop -i com.lab126.appmgrd startdefault 1 || log "Could not request the native library"
        fi
        DISPLAY=:0 xrefresh || log "Could not request an X repaint"
        attempt=0
        while ! library_ready; do
            attempt=$((attempt + 1))
            if [ "$attempt" -ge 90 ]; then
                log "Native library did not become visible and free of modal alerts"
                return 1
            fi
            sleep 1
        done
        rm -f "$session/gui" "$session/handoff"
        log "Native library window confirmed"
    fi
    rm -f "$session/dashboard" "$session/ready" "$session/beat" "$session/child" "$session/owner" "$session/framebuffer" "$session/slint-kindle-default.ttf" "$session/previous-frontlight.new" "$session/previous-wifi.new"
    rmdir "$session" || return 1
    log "Dashboard session cleaned up"
}

if [ "$mode" = launch ]; then
    [ "$(id -u)" = 0 ] || fail "Scriptlet must run as root"
    command -v setsid >/dev/null 2>&1 || fail "setsid unavailable"
    # A distinct session and redirected descriptors let the launcher outlive the GUI.
    setsid /bin/sh "$root/supervisor.sh" run "$panel_mode" </dev/null >>"$root/session.log" 2>&1 &
    exit 0
fi

[ "$(id -u)" = 0 ] || fail "Must run as root"
# Resolve an existing session first so recovery uses the same directory.
session=
for candidate in /var/tmp /dev/shm; do
    if [ -d "$candidate/kindle-dashboard-session" ]; then
        session="$candidate/kindle-dashboard-session"
        break
    fi
done

if [ "$mode" = recover ]; then
    [ -n "$session" ] || exit 0
    if [ -r "$session/owner" ] &&
        read -r owner identity < "$session/owner" && owned_process "$owner" "$identity"; then
        log "Asking the active launcher to restore the library"
        kill -TERM "$owner"
        exit $?
    fi
    log "Recovering a session whose launcher is gone"
    restore_session || fail "Recovery incomplete; inspect this log and retry"
    exit 0
fi

[ "$mode" = run ] || fail "Expected launch, run or recover"
case "$panel_mode" in panel|rtc) ;; *) fail "Unknown mode $panel_mode" ;; esac
[ -z "$session" ] || fail "Existing session. Use the recovery command after a failed session"
for program in initctl lipc-get-prop lipc-set-prop awk stat xwininfo xrefresh pidof timeout; do
    command -v "$program" >/dev/null 2>&1 || fail "Missing $program"
done
ram=$(ram_directory) || fail "No executable tmpfs found at /var/tmp or /dev/shm"
session="$ram/kindle-dashboard-session"
umask 077
mkdir "$session" || fail "Could not acquire the session directory"
trap 'status=$?; trap - 0; trap "" TERM INT HUP; restore_session || status=1; exit "$status"' 0
trap 'exit 1' TERM INT HUP
identity=$(process_id "$$")
[ -n "$identity" ] && printf '%s %s\n' "$$" "$identity" > "$session/owner" ||
    fail "Could not record session owner"

if [ "$panel_mode" = panel ]; then
    [ -s "$root/kindle-dashboard" ] || fail "Could not find Kindle executable"
    cp "$root/kindle-dashboard" "$session/dashboard" &&
        chmod 700 "$session/dashboard" || fail "Could not stage the executable"
else
    # Check the probe prerequisites before changing any native state.
    /bin/sh "$root/rtc-probe.sh" check "$root" "$session" || fail "RTC preflight failed"
fi

# Let SH Integration finish returning focus to the book library first.
for attempt in 1 2 3 4 5 6 7 8 9 10; do
    library_ready && break
    sleep 1
done
library_ready || fail "Native library is not ready for takeover"
cat /dev/fb0 > "$session/framebuffer" && [ -s "$session/framebuffer" ] ||
    fail "Could not save the native framebuffer"
previous=$(lipc-get-prop "$powerd" preventScreenSaver) || fail "Unable to read native power setting"
case "$previous" in 0|1) ;; *) fail "Unexpected preventScreenSaver value: $previous" ;; esac
printf '%s\n' "$previous" > "$session/previous-screen-saver" || fail "Could not journal power setting"
lipc-set-prop -i "$powerd" preventScreenSaver 1 &&
    [ "$(lipc-get-prop "$powerd" preventScreenSaver)" = 1 ] || fail "Could not prevent the screensaver"

touch "$session/handoff" "$session/pillow" || fail "Could not journal native handoff"
lipc-set-prop com.lab126.pillow disableEnablePillow disable || fail "Could not disable native dialogs"
manager_pid=$(pidof awesome) && pause_process "$manager_pid" "$session/awesome" ||
    fail "Could not pause the native window manager"
statusbar_job=$(initctl status statusbar 2>/dev/null || :)
case "$statusbar_job" in
    'statusbar start/running, process '*)
        pause_process "${statusbar_job##* }" "$session/statusbar" || fail "Could not pause the native status bar"
        ;;
esac
log "Paused the native window manager; framework remains running"

if [ "$panel_mode" = panel ]; then
    export TMPDIR="$session" DASHBOARD_SESSION="$session" DASHBOARD_CONFIG_ROOT="$root"
    DASHBOARD_READY="$session/ready" DASHBOARD_HEARTBEAT="$session/beat" "$session/dashboard" &
else
    /bin/sh "$root/rtc-probe.sh" run "$root" "$session" &
fi
child=$!
child_id=$(process_id "$child")
if [ -n "$child_id" ]; then
    if ! printf '%s %s\n' "$child" "$child_id" > "$session/child"; then
        kill -TERM "$child" 2>/dev/null || :
        wait "$child" || :
        fail "Could not journal dashboard child"
    fi
fi

if [ "$panel_mode" = panel ]; then
    for attempt in 1 2 3 4 5 6 7 8 9 10; do
        [ -f "$session/ready" ] && break
        owned_process "$child" "$child_id" || break
        sleep 1
    done
    [ -f "$session/ready" ] || fail "Slint failed to become ready"
    log "Dashboard ready"
    missed=0
    seen=
    while owned_process "$child" "$child_id"; do
        stamp=$(stat -c %Y "$session/beat" 2>/dev/null) || stamp=
        if [ "$stamp" = "$seen" ]; then
            missed=$((missed + 1))
            [ "$missed" -lt 20 ] || fail "Dashboard stopped responding"
        else
            missed=0
            seen=$stamp
        fi
        sleep 1
    done
fi

wait "$child"
status=$?
log "$panel_mode exited with status $status"
exit "$status"
