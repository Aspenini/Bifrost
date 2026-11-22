#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    printf 'Usage: sh %s HOST_BIFROST GUEST_IMAGE\n' "$0" >&2
    exit 2
fi

runner=$(realpath -- "$1")
image=$(realpath -- "$2")
if [ ! -f "$runner" ] || [ ! -x "$runner" ] || [ ! -f "$image" ]; then
    printf 'Expected an executable Linux Bifrost runner and a regular guest image\n' >&2
    exit 2
fi

exec bwrap --unshare-all --unshare-user --disable-userns --cap-drop ALL \
    --die-with-parent --new-session --clearenv \
    --ro-bind /usr /usr --ro-bind-try /lib /lib --ro-bind-try /lib64 /lib64 \
    --dev /dev --proc /proc --ro-bind "$runner" /bifrost --ro-bind "$image" /guest \
    --chdir / -- /bifrost run --exec interp /guest </dev/null
