# Sourced by the scripts in this folder. Defines load_env, which reads the KEY=value lines of
# a .env file into the environment. Variables that are already set keep their value, so a
# setting on the command line wins over the file.
#
# The file is read literally rather than sourced, so the backslashes in a Windows path stay as
# typed. Blank lines and lines starting with # are skipped, and one pair of quotes around a
# value is removed.
load_env() {
  local file="$1" line key value
  [ -f "$file" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    case "$line" in '' | '#'*) continue ;; esac
    key="${line%%=*}"
    value="${line#*=}"
    [[ "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
    case "$value" in
      \"*\" | \'*\') value="${value:1:${#value}-2}" ;;
    esac
    [ -n "${!key+x}" ] || export "$key=$value"
  done < "$file"
}
