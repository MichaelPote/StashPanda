# Run by `ssh_check.rs` as a Git alias (`git -c alias.stashpanda-ssh-check='!<this>'`), so it
# runs in the shell Git starts for its own commands: with Git's PATH (Git for Windows' own ssh.exe
# rather than Windows OpenSSH), Git's environment and the repository's config. Every line of
# output that starts with `@@` names what follows it.
#
# Arguments: `setup` or `test`, then `user@host`, then the port or an empty string.
f() {
  mode="$1"
  dest="$2"
  port="$3"
  # The SSH command Git itself runs, in Git's order of precedence.
  if [ -n "${GIT_SSH_COMMAND:-}" ]; then
    sshcmd="$GIT_SSH_COMMAND"
    src=GIT_SSH_COMMAND
  elif [ -n "${GIT_SSH:-}" ]; then
    sshcmd="\"$GIT_SSH\""
    src=GIT_SSH
  else
    sshcmd=$(git config core.sshCommand)
    if [ -n "$sshcmd" ]; then src=core.sshCommand; else sshcmd=ssh; src=default; fi
  fi
  portarg=
  [ -n "$port" ] && portarg="-p $port"
  if [ "$mode" = test ]; then
    echo "@@command $sshcmd -T -v $portarg $dest"
    eval "$sshcmd -T -v -o BatchMode=yes -o ConnectTimeout=10 $portarg \"\$dest\"" </dev/null 2>&1
    echo "@@exit $?"
    return 0
  fi
  # A Windows path for a program in Git for Windows' shell, which reports `/usr/bin/ssh`.
  disp() {
    if [ -n "$1" ] && command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s\n' "$1"; fi
  }
  eval "set -- $sshcmd"
  prog="$1"
  # ssh-add and ssh-keygen from the same folder as the ssh program, when it was given as a path.
  case "$prog" in
    */* | *\\*) dir="$(dirname "$(printf '%s' "$prog" | tr '\\' /)")/" ;;
    *) dir= ;;
  esac
  echo "@@git $(disp "$(command -v git)")"
  echo "@@gitversion $(git --version)"
  echo "@@source $src"
  echo "@@sshcommand $sshcmd"
  echo "@@program $(disp "$(command -v "$prog")")"
  echo "@@version $(eval "$sshcmd -V" </dev/null 2>&1 | head -n 1)"
  echo "@@config"
  eval "$sshcmd -G $portarg \"\$dest\"" </dev/null 2>/dev/null | while read -r k v; do
    case "$k" in
      identityfile)
        case "$v" in "~/"*) v="$HOME/${v#??}" ;; esac
        # present, missing, encrypted (an old PEM key with no .pub beside it, whose fingerprint
        # needs the passphrase) or unreadable, then the path, a tab and ssh-keygen's line.
        fp=
        if [ ! -f "$v" ]; then
          state=missing
        else
          kf="$v"
          [ -f "$v.pub" ] && kf="$v.pub"
          if fp=$("${dir}ssh-keygen" -lf "$kf" </dev/null 2>/dev/null | head -n 1) && [ -n "$fp" ]; then
            state=present
          elif grep -q ENCRYPTED "$v" 2>/dev/null; then
            state=encrypted
          else
            state=unreadable
          fi
        fi
        printf 'identityfile %s %s\t%s\n' "$state" "$(disp "$v")" "$fp"
        ;;
      user | hostname | port | identitiesonly | identityagent) echo "$k $v" ;;
    esac
  done
  echo "@@agent"
  "${dir}ssh-add" -l </dev/null 2>&1
  echo "@@agentexit $?"
}
f