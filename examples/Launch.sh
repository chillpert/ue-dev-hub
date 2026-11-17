#!/usr/bin/env bash
# Opens this project in UE Dev Hub. Put this file in the project root, next to the .uproject.
# Runs on Linux and, through Git Bash, on Windows (double-click it, or run ./Launch.sh).
#
# Uses an installed hub if there is one. Otherwise it runs a copy that isn't installed,
# downloading it the first time. Nothing gets installed: on Windows the hub asks whether to
# install it, and it updates itself either way.
set -euo pipefail

# The GitHub repository the hub's releases come from.
REPO="chillpert/ue-dev-hub"
PROJECT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) WINDOWS=1 ;;
    *) WINDOWS=0 ;;
esac

# Downloads the release file $1 to $2. Exits with a message if there's nothing to download.
download() {
    echo "Downloading UE Dev Hub ..."
    mkdir -p "$(dirname "$2")"
    local status
    status=$(curl -sL -w '%{http_code}' -o "$2.download" "https://github.com/$REPO/releases/latest/download/$1" || echo 000)
    if [ "$status" != "200" ]; then
        rm -f "$2.download"
        if [ "$status" = "404" ]; then
            echo "There's no UE Dev Hub release to download yet, or its GitHub repository is private."
            echo "Ask your team for the hub."
        else
            echo "Couldn't download UE Dev Hub. Check your internet connection and run this again."
        fi
        read -rp "Press Enter to close." _ || true
        exit 1
    fi
    mv "$2.download" "$2"
}

if [ "$WINDOWS" = 1 ]; then
    LOCAL="$(cygpath -u "$LOCALAPPDATA")"
    INSTALLED="$LOCAL/UE Dev Hub/ue-dev-hub.exe"
    # Kept outside the project: Windows looks for programs and libraries next to a running .exe
    # first, so a copy inside a repository could pick up files that the repository contains.
    PORTABLE="$LOCAL/UE Dev Hub Portable/ue-dev-hub.exe"
    EXE="$INSTALLED"
    if [ ! -f "$EXE" ]; then
        EXE="$PORTABLE"
        [ -f "$EXE" ] || download ue-dev-hub.exe "$EXE"
    fi
    # `start` detaches the hub, so this window can close.
    cmd //c start "" "$(cygpath -w "$EXE")" --project "$(cygpath -w "$PROJECT")"
else
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    if command -v ue-dev-hub >/dev/null 2>&1; then
        EXE="$(command -v ue-dev-hub)"
    else
        # The AppImage updates itself.
        EXE="$DATA/ue-dev-hub/ue-dev-hub.AppImage"
        if [ ! -x "$EXE" ]; then
            download ue-dev-hub.AppImage "$EXE"
            chmod +x "$EXE"
        fi
        # AppImages mount themselves with FUSE 2, which newer distributions don't install. Without
        # it, the AppImage unpacks itself to a temporary folder on each start instead.
        LIBS="$(ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null || true)"
        case "$LIBS" in
            *libfuse.so.2*) EXTRACT="" ;;
            *) EXTRACT="env APPIMAGE_EXTRACT_AND_RUN=1 "; export APPIMAGE_EXTRACT_AND_RUN=1 ;;
        esac
        # A menu entry, so the hub can be found without this script. Made once.
        DESKTOP="$DATA/applications/ue-dev-hub.desktop"
        if [ ! -f "$DESKTOP" ]; then
            mkdir -p "$DATA/applications" "$DATA/icons/hicolor/128x128/apps"
            TMP="$(mktemp -d)"
            # Takes the icon out of the AppImage, which works without FUSE.
            (cd "$TMP" && "$EXE" --appimage-extract '*.png' >/dev/null 2>&1) || true
            ICON="$(find "$TMP/squashfs-root" -maxdepth 1 -name '*.png' 2>/dev/null | head -n 1 || true)"
            if [ -n "$ICON" ]; then
                cp "$ICON" "$DATA/icons/hicolor/128x128/apps/ue-dev-hub.png" || true
            fi
            rm -rf "$TMP"
            cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=UE Dev Hub
Comment=Update, build and launch Unreal Engine projects
Exec=${EXTRACT}"$EXE"
Icon=ue-dev-hub
Categories=Development;
Terminal=false
EOF
        fi
    fi
    # The hub's output, for when it doesn't start.
    LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/ue-dev-hub"
    mkdir -p "$LOG_DIR"
    LOG="$LOG_DIR/launch.log"
    nohup "$EXE" --project "$PROJECT" >"$LOG" 2>&1 &
    PID=$!
    sleep 1
    # Gone after a second: it either handed the project to a hub that's already open, which
    # is fine, or it failed to start.
    if ! kill -0 "$PID" 2>/dev/null && ! wait "$PID"; then
        echo "UE Dev Hub didn't start. Its output (also in $LOG):"
        echo
        tail -n 30 "$LOG"
        echo
        if grep -qi "fuse" "$LOG"; then
            echo "Install FUSE 2 (for example: sudo apt install libfuse2t64, or libfuse2 on older systems) and try again."
        else
            echo "It needs WebKitGTK 4.1 and GTK 3 (for example: sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0)."
        fi
        read -rp "Press Enter to close." _ || true
        exit 1
    fi
fi
