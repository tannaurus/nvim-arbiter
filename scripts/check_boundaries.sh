#!/usr/bin/env bash
# Module boundary enforcement per RFD stream isolation rules.
# Verifies prohibited imports between streams.

set -euo pipefail

ERRORS=0
SRC="crates/arbiter/src"

check_no_import() {
    local module="$1"
    local forbidden="$2"
    if rg -l "use crate::${forbidden}" "$SRC/$module" 2>/dev/null | grep -q .; then
        echo "ERROR: $module imports $forbidden"
        ERRORS=$((ERRORS + 1))
    fi
}

# Stream 1 (git, diff) must not import threads, backend, review, poll, turn
for mod in git.rs diff/; do
    for forbidden in threads backend review poll turn; do
        check_no_import "$mod" "$forbidden"
    done
done

# Stream 2 (threads, state) must not import git, diff, backend, review, poll, turn
for mod in threads/ state.rs; do
    for forbidden in git diff backend review poll turn; do
        check_no_import "$mod" "$forbidden"
    done
done

# Stream 3 (backend) must not import threads, diff, review, git, poll, turn
for mod in backend/; do
    for forbidden in threads diff review git poll turn; do
        check_no_import "$mod" "$forbidden"
    done
done

exit $ERRORS
