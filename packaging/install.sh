#!/bin/sh
# Builds and installs Silhouette Studio for Linux on any distribution.
#
#   sh packaging/install.sh            # system-wide into /usr/local (asks for sudo)
#   sh packaging/install.sh --user     # just for you, into ~/.local (no sudo, no udev rule)
#   sh packaging/install.sh --prefix /opt/silhouette-studio
#   sh packaging/install.sh --uninstall [--user | --prefix DIR]
#
# Options: --no-build (install an existing target/release build), --no-udev.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=/usr/local
USER_INSTALL=0
BUILD=1
UDEV=1
UNINSTALL=0

while [ $# -gt 0 ]; do
    case "$1" in
        --user) USER_INSTALL=1; PREFIX="$HOME/.local"; UDEV=0 ;;
        --prefix) shift; PREFIX=$1 ;;
        --no-build) BUILD=0 ;;
        --no-udev) UDEV=0 ;;
        --uninstall) UNINSTALL=1 ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

# Use sudo only when the destination is not writable by the current user.
SUDO=
probe=$PREFIX
while [ ! -e "$probe" ]; do probe=$(dirname "$probe"); done
if [ ! -w "$probe" ] && [ "$(id -u)" -ne 0 ]; then SUDO=sudo; fi

BIN="$PREFIX/bin"
SHARE="$PREFIX/share"
UDEV_RULE=/etc/udev/rules.d/70-silhouette-cutters.rules
FILES="$BIN/silhouette-studio $BIN/ss-cli $SHARE/applications/silhouette-studio.desktop \
$SHARE/icons/hicolor/scalable/apps/silhouette-studio.svg $SHARE/mime/packages/silhouette-studio.xml"

refresh_caches() {
    command -v update-desktop-database >/dev/null 2>&1 && $SUDO update-desktop-database -q "$SHARE/applications" || true
    command -v update-mime-database >/dev/null 2>&1 && $SUDO update-mime-database "$SHARE/mime" >/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && $SUDO gtk-update-icon-cache -q -t "$SHARE/icons/hicolor" || true
}

if [ "$UNINSTALL" -eq 1 ]; then
    # shellcheck disable=SC2086
    $SUDO rm -f $FILES
    if [ "$UDEV" -eq 1 ] && [ -e "$UDEV_RULE" ]; then
        [ "$(id -u)" -eq 0 ] && rm -f "$UDEV_RULE" || sudo rm -f "$UDEV_RULE"
    fi
    refresh_caches
    echo "Removed Silhouette Studio from $PREFIX"
    exit 0
fi

if [ "$BUILD" -eq 1 ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo not found. Install Rust 1.88 or newer from https://rustup.rs and run this again." >&2
        exit 1
    fi
    (cd "$ROOT" && cargo build --release --locked -p ss-app -p ss-cli)
fi
[ -x "$ROOT/target/release/silhouette-studio" ] || { echo "no build found in $ROOT/target/release" >&2; exit 1; }

$SUDO install -Dm755 "$ROOT/target/release/silhouette-studio" "$BIN/silhouette-studio"
$SUDO install -Dm755 "$ROOT/target/release/ss-cli" "$BIN/ss-cli"
$SUDO install -Dm644 "$ROOT/packaging/silhouette-studio.desktop" "$SHARE/applications/silhouette-studio.desktop"
$SUDO install -Dm644 "$ROOT/packaging/silhouette-studio.svg" "$SHARE/icons/hicolor/scalable/apps/silhouette-studio.svg"
$SUDO install -Dm644 "$ROOT/packaging/silhouette-studio.xml" "$SHARE/mime/packages/silhouette-studio.xml"
refresh_caches

if [ "$UDEV" -eq 1 ]; then
    R=sudo; [ "$(id -u)" -eq 0 ] && R=
    $R install -Dm644 "$ROOT/packaging/70-silhouette-cutters.rules" "$UDEV_RULE"
    if command -v udevadm >/dev/null 2>&1; then
        $R udevadm control --reload-rules && $R udevadm trigger || true
    fi
    echo "Installed the cutter udev rule. Unplug and replug the cutter once."
elif [ "$USER_INSTALL" -eq 1 ]; then
    echo "Note: a --user install cannot add the USB permission rule. To use a cutter, run once:"
    echo "  sudo install -Dm644 $ROOT/packaging/70-silhouette-cutters.rules $UDEV_RULE && sudo udevadm control --reload-rules"
fi

echo "Installed Silhouette Studio to $PREFIX"
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Add $BIN to your PATH to run silhouette-studio from a terminal." ;; esac
