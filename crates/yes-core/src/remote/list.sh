set -euo pipefail
export LC_ALL=C
fail() { printf '%s\n' "$*" >&2; exit 1; }
for tool in find stat head tail base64 tr wc; do
    command -v "$tool" >/dev/null 2>&1 || fail "Required remote tool is unavailable: $tool"
done
root=$1
case "$root" in
    '~') root=$HOME ;;
    '~/'*) root=$HOME/${root#\~/} ;;
    /*) ;;
    *) fail 'Codex root must be absolute or start with ~/' ;;
esac
cd -P -- "$root" || fail 'Cannot open Codex root'
root=$(pwd -P)
[ ! -L sessions ] || fail 'Codex sessions directory must not be a symbolic link'
[ -d sessions ] || fail 'Codex sessions directory does not exist'
count=$(find -P ./sessions -type f -name '*.jsonl' -exec /bin/bash -c 'for file do printf x; done' yes-sessions {} + | wc -c)
[ "$count" -le 2000 ] || fail 'Remote Codex listing exceeds the 2000-file limit'
printf 'ROOT\t'
printf '%s' "$root" | base64 | tr -d '\n'
printf '\n'
if [ -e session_index.jsonl ] || [ -L session_index.jsonl ]; then
    [ ! -L session_index.jsonl ] && [ -f session_index.jsonl ] || fail 'Codex index must be a regular file, not a symbolic link'
    [ "$(stat -c %s -- session_index.jsonl)" -le 1048576 ] || fail 'Codex session index exceeds the 1 MiB limit'
    printf 'INDEX\t'
    head -c 1048576 -- session_index.jsonl | base64 | tr -d '\n'
    printf '\n'
fi
find -P ./sessions -type f -name '*.jsonl' -exec /bin/bash -c '
    set -euo pipefail
    for file do
        [ ! -L "$file" ] && [ -f "$file" ] || exit 1
        metadata=$(stat -c "%Y %s" -- "$file")
        printf "FILE\t"
        printf "%s" "${file#./}" | base64 | tr -d "\n"
        set -- $metadata
        printf "\t%s\t%s\t" "$1" "$2"
        head -c 262144 -- "$file" | base64 | tr -d "\n"
        printf "\t"
        tail -c 65536 -- "$file" | base64 | tr -d "\n"
        printf "\n"
    done
' yes-sessions {} +

printf "END\n"
