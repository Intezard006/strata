#!/bin/sh
# Toggle a "textbook" look in the running strata (the focused one, else the newest):
# original page colours and no statusbar, versus the recolored default.
# Usage: textbook-toggle.sh [on|off]      (no argument toggles)
# Later this can switch per-book profiles; for now it only sets options.
state="${XDG_RUNTIME_DIR:-/tmp}/strata-textbook"
want="$1"
if [ -z "$want" ]; then
    if [ -e "$state" ]; then want=off; else want=on; fi
fi
case "$want" in
    on)
        strata --exec 'set recolor false' &&
        strata --exec 'set statusbar false' &&
        : > "$state"
        ;;
    off)
        strata --exec 'set recolor true' &&
        strata --exec 'set statusbar true' &&
        rm -f "$state"
        ;;
    *)
        echo "usage: $0 [on|off]" >&2
        exit 2
        ;;
esac
