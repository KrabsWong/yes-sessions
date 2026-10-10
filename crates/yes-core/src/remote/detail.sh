set -euo pipefail
export LC_ALL=C
fail() { printf '%s\n' "$*" >&2; exit 1; }
for tool in stat head base64 tr; do
    command -v "$tool" >/dev/null 2>&1 || fail "Required remote tool is unavailable: $tool"
done
root=$1
shift
case "$root" in
    '~') root=$HOME ;;
    '~/'*) root=$HOME/${root#\~/} ;;
    /*) ;;
    *) fail 'Codex root must be absolute or start with ~/' ;;
esac
cd -P -- "$root" || fail 'Cannot open Codex root'
root=$(pwd -P)
printf 'ROOT\t'
printf '%s' "$root" | base64 | tr -d '\n'
printf '\n'
for file do
    case "$file" in
        sessions/*.jsonl) ;;
        *) fail 'Invalid Codex session path' ;;
    esac
    case "/$file/" in
        */../*|*/./*|*//*) fail 'Codex session path contains traversal components' ;;
    esac
    remaining=$file
    component_path=.
    while :; do
        component=${remaining%%/*}
        component_path=$component_path/$component
        [ ! -L "$component_path" ] || fail 'Codex session path contains a symbolic link'
        case "$remaining" in
            */*) remaining=${remaining#*/} ;;
            *) break ;;
        esac
    done
    [ -f "$file" ] || fail 'Codex session file does not exist'
    size=$(stat -c %s -- "$file")
    [ "$size" -le 33554432 ] || fail 'Remote Codex session exceeds the 32 MiB limit'
    modified=$(stat -c %Y -- "$file")
    printf 'FILE\t'
    printf '%s' "$file" | base64 | tr -d '\n'
    printf '\t%s\t%s\t' "$modified" "$size"
    head -c "$size" -- "$file" | base64 | tr -d '\n'
    printf '\n'
done

printf "END\n"
