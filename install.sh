#!/usr/bin/env bash
set -euo pipefail
umask 022

BINARY_NAME="hashline"
OWNER="quangdang46"
REPO="hashline"
DEST="${DEST:-$HOME/.local/bin}"
VERSION="${VERSION:-}"
QUIET=0
EASY=0
VERIFY=0
FROM_SOURCE=0
UNINSTALL=0
MAX_RETRIES=3
DOWNLOAD_TIMEOUT=120
LOCK_DIR="/tmp/${BINARY_NAME}-install.lock.d"
TMP=""

log_info() {
    [ "$QUIET" -eq 1 ] && return
    echo "[${BINARY_NAME}] $*" >&2
}

log_warn() {
    echo "[${BINARY_NAME}] WARN: $*" >&2
}

log_success() {
    [ "$QUIET" -eq 1 ] && return
    echo "✓ $*" >&2
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

sed_in_place() {
    local expr="$1"
    local file="$2"

    if sed --version >/dev/null 2>&1; then
        sed -i "$expr" "$file"
    else
        sed -i '' "$expr" "$file"
    fi
}

usage() {
    cat <<'EOF'
Install hashline from GitHub releases.

Usage:
  install.sh [options]

Options:
  --dest PATH         Install into PATH
  --dest=PATH         Install into PATH
  --version VERSION   Install a specific release tag
  --version=VERSION   Install a specific release tag
  --system            Install into /usr/local/bin
  --easy-mode         Append DEST to shell rc files if needed
  --verify            Run --version after install
  --from-source       Build from source instead of downloading assets
  --quiet, -q         Reduce output
  --uninstall         Remove installed binary
  -h, --help          Show this help
EOF
    exit 0
}

cleanup() {
    rm -rf "$TMP" "$LOCK_DIR" 2>/dev/null || true
}

trap cleanup EXIT

acquire_lock() {
    if mkdir "$LOCK_DIR" 2>/dev/null; then
        echo $$ > "$LOCK_DIR/pid"
        return 0
    fi
    die "Another install is running. If stuck: rm -rf $LOCK_DIR"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dest)
            [ $# -ge 2 ] || die "Missing value for --dest"
            DEST="$2"
            shift 2
            ;;
        --dest=*)
            DEST="${1#*=}"
            shift
            ;;
        --version)
            [ $# -ge 2 ] || die "Missing value for --version"
            VERSION="$2"
            shift 2
            ;;
        --version=*)
            VERSION="${1#*=}"
            shift
            ;;
        --system)
            DEST="/usr/local/bin"
            shift
            ;;
        --easy-mode)
            EASY=1
            shift
            ;;
        --verify)
            VERIFY=1
            shift
            ;;
        --from-source)
            FROM_SOURCE=1
            shift
            ;;
        --quiet|-q)
            QUIET=1
            shift
            ;;
        --uninstall)
            UNINSTALL=1
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            die "Unknown argument: $1"
            ;;
    esac
done

do_uninstall() {
    rm -f "$DEST/$BINARY_NAME" "$DEST/$BINARY_NAME.exe"
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        [ -f "$rc" ] && sed_in_place "/${BINARY_NAME} installer/d" "$rc" 2>/dev/null || true
    done
    log_success "Uninstalled"
    exit 0
}

[ "$UNINSTALL" -eq 1 ] && do_uninstall

detect_platform() {
    local os arch
    case "$(uname -s)" in
        Linux*)
            os="linux"
            ;;
        Darwin*)
            os="macos"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            os="windows"
            ;;
        *)
            die "Unsupported OS: $(uname -s)"
            ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)
            arch="x86_64"
            ;;
        aarch64|arm64)
            arch="aarch64"
            ;;
        *)
            die "Unsupported arch: $(uname -m)"
            ;;
    esac

    echo "${os}_${arch}"
}

archive_name_for_platform() {
    local platform="$1"
    local suffix ext
    case "$platform" in
        linux_x86_64)
            suffix="linux-x86_64"
            ext="tar.gz"
            ;;
        linux_aarch64)
            suffix="linux-aarch64"
            ext="tar.gz"
            ;;
        macos_x86_64)
            suffix="macos-x86_64"
            ext="tar.gz"
            ;;
        macos_aarch64)
            suffix="macos-aarch64"
            ext="tar.gz"
            ;;
        windows_x86_64)
            suffix="windows-x86_64"
            ext="zip"
            ;;
        *)
            die "Unsupported platform: $platform"
            ;;
    esac

    printf '%s-%s-%s.%s' "$BINARY_NAME" "$VERSION" "$suffix" "$ext"
}

# === Version ===
FULL_TAG=""
resolve_version() {
    [ -n "$VERSION" ] && return 0
    FULL_TAG=$(curl -fsSL --connect-timeout 10 --max-time 30 \
        "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" 2>/dev/null \
        | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/') || true
    if ! [[ "$FULL_TAG" =~ v[0-9] ]]; then
        FULL_TAG=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
            "https://github.com/${OWNER}/${REPO}/releases/latest" 2>/dev/null \
            | sed -E 's|.*/tag/||') || true
    fi
    [[ "$FULL_TAG" =~ v[0-9] ]] || die "Could not resolve version"

    if [[ "$FULL_TAG" =~ (v[0-9]+\.[0-9]+\.[0-9]+.*)$ ]]; then
       VERSION="${BASH_REMATCH[1]}"
    else
       VERSION="$FULL_TAG"
    fi

    log_info "Latest: $FULL_TAG ($VERSION)"
}

download_file() {
    local url="$1"
    local dest="$2"
    local partial="${dest}.part"
    local attempt=0

    while [ $attempt -lt $MAX_RETRIES ]; do
        attempt=$((attempt + 1))
        if curl -fL \
            --connect-timeout 30 \
            --max-time "$DOWNLOAD_TIMEOUT" \
            --retry 2 \
            $( [ -s "$partial" ] && echo "--continue-at -" ) \
            $( [ "$QUIET" -eq 0 ] && [ -t 2 ] && echo "--progress-bar" || echo "-sS" ) \
            -o "$partial" "$url"; then
            mv -f "$partial" "$dest"
            return 0
        fi

        if [ $attempt -lt $MAX_RETRIES ]; then
            log_warn "Retrying in 3s..."
            sleep 3
        fi
    done

    return 1
}

checksum_file() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    else
        shasum -a 256 "$file" | awk '{print $1}'
    fi
}

verify_checksum() {
    local archive_path="$1"
    local checksum_path="$2"
    local expected actual

    expected=$(awk '{print $1}' "$checksum_path")
    actual=$(checksum_file "$archive_path")
    [ "$expected" = "$actual" ] || die "Checksum mismatch"
}

install_binary_atomic() {
    local src="$1"
    local dest="$2"
    local tmp="${dest}.tmp.$$"

    install -m 0755 "$src" "$tmp"
    mv -f "$tmp" "$dest" || {
        rm -f "$tmp"
        die "Failed to install binary"
    }
}

maybe_add_path() {
    case ":$PATH:" in
        *":$DEST:"*)
            return 0
            ;;
    esac

    if [ "$EASY" -eq 1 ]; then
        for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
            [ -f "$rc" ] && [ -w "$rc" ] || continue
            grep -qF "$DEST" "$rc" && continue
            printf '\nexport PATH="%s:$PATH"  # %s installer\n' "$DEST" "$BINARY_NAME" >> "$rc"
        done
        log_warn "PATH updated — restart shell or: export PATH=\"$DEST:\$PATH\""
    else
        log_warn "Add to PATH: export PATH=\"$DEST:\$PATH\""
    fi
}

build_from_source() {
    command -v cargo >/dev/null || die "Rust/cargo not found. Install: https://rustup.rs"
    command -v git >/dev/null || die "git not found"
    git clone --depth 1 "https://github.com/${OWNER}/${REPO}.git" "$TMP/src"
    (
        cd "$TMP/src"
        CARGO_TARGET_DIR="$TMP/target" cargo build --release --locked -p "$BINARY_NAME"
    )
    install_binary_atomic "$TMP/target/release/$BINARY_NAME" "$DEST/$BINARY_NAME"
}

extract_archive() {
    local archive="$1"
    case "$archive" in
        *.tar.gz)
            tar -xzf "$archive" -C "$TMP"
            ;;
        *.zip)
            unzip -q "$archive" -d "$TMP"
            ;;
        *)
            die "Unsupported archive format: $archive"
            ;;
    esac
}

find_extracted_binary() {
    local candidate
    if [[ "$1" == windows_* ]]; then
        candidate=$(find "$TMP" -type f -name "$BINARY_NAME.exe" 2>/dev/null | head -n 1)
    else
        candidate=$(find "$TMP" -type f -name "$BINARY_NAME" -perm -111 2>/dev/null | head -n 1)
    fi
    [ -n "$candidate" ] || die "Binary not found after extraction"
    printf '%s\n' "$candidate"
}

run_mcp_auto_install() {
    log_info "Auto-installing MCP provider configs..."

    local bin="$DEST/$BINARY_NAME"
    # Resolve to absolute, canonical path so configs survive PATH changes.
    local abs_bin
    if abs_bin=$(readlink -f "$bin" 2>/dev/null) && [ -n "$abs_bin" ]; then
        bin="$abs_bin"
    fi

    local results=()
    local any_installed=0

    # ---- JSON hosts (require jq for safe upsert) ----
    if ! command -v jq >/dev/null 2>&1; then
        log_warn "jq not found — skipping JSON-based MCP host configs"
        log_warn "Install jq (https://jqlang.org) and re-run install.sh, or configure MCP manually"
    else
        # host_name|config_path|servers_key|note
        local json_hosts=(
            "claude-code|$HOME/.claude.json|mcpServers|User scope"
            "cursor|$HOME/.cursor/mcp.json|mcpServers|Global scope"
            "windsurf|$HOME/.codeium/windsurf/mcp_config.json|mcpServers|Global scope"
            "vscode|$PWD/.vscode/mcp.json|servers|Project scope"
            "gemini|$HOME/.gemini/settings.json|mcpServers|User scope"
            "opencode|$HOME/.opencode.json|mcpServers|User scope"
            "amp|$HOME/.config/amp/settings.json|amp.mcpServers|User scope"
            "droid|$HOME/.factory/mcp.json|mcpServers|User scope"
        )

        local host_entry name path key note detect_root
        for host_entry in "${json_hosts[@]}"; do
            IFS='|' read -r name path key note <<<"$host_entry"

            # Skip hosts whose owning dir/file does not exist (mirrors detect_hosts in old Rust code)
            case "$name" in
                claude-code) [ -e "$HOME/.claude.json" ] || continue ;;
                cursor) [ -d "$HOME/.cursor" ] || continue ;;
                windsurf) [ -d "$HOME/.codeium/windsurf" ] || continue ;;
                vscode) [ -d "$PWD/.vscode" ] || continue ;;
                gemini) [ -d "$HOME/.gemini" ] || continue ;;
                opencode) [ -e "$HOME/.opencode.json" ] || continue ;;
                amp) [ -d "$HOME/.config/amp" ] || continue ;;
                droid) [ -d "$HOME/.factory" ] || continue ;;
            esac

            mkdir -p "$(dirname "$path")"

            local existing='{}' status='installed'
            if [ -f "$path" ] && [ -s "$path" ]; then
                if existing=$(jq '.' "$path" 2>/dev/null); then
                    # Check if entry already exists and is identical
                    local current
                    current=$(printf '%s' "$existing" | jq --arg key "$key" '.[$key]["'"$BINARY_NAME"'"] // empty' 2>/dev/null || echo '')
                    if [ -n "$current" ]; then
                        local desired
                        desired=$(jq -n --arg cmd "$bin" '{command: $cmd, args: ["mcp"]}')
                        if [ "$current" = "$desired" ]; then
                            status='unchanged'
                        else
                            status='updated'
                        fi
                    fi
                else
                    log_warn "$name: invalid JSON in $path — skipped"
                    continue
                fi
            fi

            if [ "$status" != 'unchanged' ]; then
                local updated
                updated=$(printf '%s' "$existing" | jq \
                    --arg key "$key" \
                    --arg name "$BINARY_NAME" \
                    --arg cmd "$bin" \
                    '.[$key] = (.[$key] // {}) | .[$key][$name] = {command: $cmd, args: ["mcp"]}'
                ) || { log_warn "$name: jq upsert failed — skipped"; continue; }
                printf '%s\n' "$updated" >"$path" || { log_warn "$name: write to $path failed"; continue; }
            fi

            results+=("- $name: $status ($path) — $note")
            any_installed=1
        done
    fi

    # ---- TOML host: codex (~/.codex/config.toml) ----
    if [ -d "$HOME/.codex" ]; then
        local codex_path="$HOME/.codex/config.toml"
        local marker="[mcp_servers.${BINARY_NAME}]"
        local section
        section=$(printf '%s\ncommand = "%s"\nargs = ["mcp"]\n' "$marker" "$bin")

        mkdir -p "$HOME/.codex"
        if [ -f "$codex_path" ] && grep -qF "$marker" "$codex_path"; then
            # Check unchanged: extract current section and compare
            local current_section
            current_section=$(awk -v m="$marker" '
                $0 == m {p=1}
                p && /^\[/ && $0 != m {p=0}
                p {print}
            ' "$codex_path")
            if [ "$(printf '%s' "$current_section" | tr -d '[:space:]')" = "$(printf '%s' "$section" | tr -d '[:space:]')" ]; then
                results+=("- codex: unchanged ($codex_path) — User scope")
            else
                # Replace existing section
                local tmp="$codex_path.tmp.$$"
                awk -v m="$marker" -v new="$section" '
                    $0 == m {p=1; print new; next}
                    p && /^\[/ && $0 != m {p=0}
                    !p {print}
                ' "$codex_path" >"$tmp" && mv "$tmp" "$codex_path"
                results+=("- codex: updated ($codex_path) — User scope")
                any_installed=1
            fi
        else
            # Append new section
            if [ -f "$codex_path" ] && [ -s "$codex_path" ]; then
                # Ensure separating newline
                tail -c1 "$codex_path" | read -r _ || printf '\n' >>"$codex_path"
                printf '\n%s\n' "$section" >>"$codex_path"
            else
                printf '%s\n' "$section" >"$codex_path"
            fi
            results+=("- codex: installed ($codex_path) — User scope")
            any_installed=1
        fi
    fi

    if [ "$any_installed" -eq 0 ] && [ "${#results[@]}" -eq 0 ]; then
        log_warn "No supported MCP providers detected."
        return 0
    fi

    log_info "${BINARY_NAME} MCP auto-install results:"
    for line in "${results[@]}"; do
        echo "$line"
    done
    return 0
}

print_summary() {
    echo ""
    echo "✓ ${BINARY_NAME} installed → $DEST/$BINARY_NAME"
    echo "  Version: $VERSION"
    echo ""
    echo "  Usage: $BINARY_NAME --help"
}

main() {
    acquire_lock
    TMP=$(mktemp -d)
    mkdir -p "$DEST"

    local platform archive url binary_path
    platform=$(detect_platform)
    log_info "Platform: $platform"
    log_info "Destination: $DEST"

    if [ "$FROM_SOURCE" -eq 0 ]; then
        resolve_version
        archive=$(archive_name_for_platform "$platform")
        url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${archive}"

        if download_file "$url" "$TMP/$archive"; then
            if download_file "${url}.sha256" "$TMP/checksum.sha256" 2>/dev/null; then
                verify_checksum "$TMP/$archive" "$TMP/checksum.sha256"
                log_info "Checksum verified"
            fi

            extract_archive "$TMP/$archive"
            binary_path=$(find_extracted_binary "$platform")
            install_binary_atomic "$binary_path" "$DEST/$BINARY_NAME"
        else
            log_warn "Binary download failed — building from source..."
            build_from_source
        fi
    else
        build_from_source
    fi

    maybe_add_path

    # Also replace any existing `hashline` on PATH at a different location
    # so `hashline --version` always shows the latest version.
    local existing_bin
    existing_bin=$(command -v "$BINARY_NAME" 2>/dev/null || true)
    if [ -n "$existing_bin" ]; then
        local existing_dir
        existing_dir=$(dirname "$existing_bin")
        if [ "$(cd "$existing_dir" && pwd)" != "$(cd "$DEST" && pwd)" ]; then
            log_info "also updating existing $BINARY_NAME at $existing_bin"
            if cp -f "$DEST/$BINARY_NAME" "$existing_bin" 2>/dev/null; then
                log_ok "replaced $existing_bin"
            else
                log_warn "could not update $existing_bin — you may need sudo or remove it manually"
            fi
        fi
    fi

    if [ "$VERIFY" -eq 1 ]; then
        "$DEST/$BINARY_NAME" --version >/dev/null
    fi

    run_mcp_auto_install || true
    print_summary
}

if [[ "${BASH_SOURCE[0]:-}" == "${0:-}" ]] || [[ -z "${BASH_SOURCE[0]:-}" ]]; then
    { main "$@"; }
fi
