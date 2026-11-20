# Runs on the execution Host. Arguments are the working directory and the
# absolute paths of the macOS system Git stub and developer directory selector.
unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR
directory=$1
system_git=$2
selector=$3
git_path=$(command -v git) || exit 0
case "$git_path" in
  */*) ;;
  *) exit 0 ;;
esac
[ -f "$git_path" ] && [ -x "$git_path" ] || exit 0

if [ "$git_path" = "$system_git" ] || [ "$git_path" -ef "$system_git" ]; then
  platform=$(uname -s) || exit 0
  if [ "$platform" = Darwin ]; then
    # xcode-select's print mode does not invoke the stub or request installation.
    developer=$("$selector" -p 2>/dev/null) || exit 0
    case "$developer" in
      /*) ;;
      *) exit 0 ;;
    esac
    git_path=$developer/usr/bin/git
    if [ ! -f "$git_path" ] || [ ! -x "$git_path" ]; then
      git_path=$developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/git
    fi
    [ -f "$git_path" ] && [ -x "$git_path" ] || exit 0
    [ "$git_path" -ef "$system_git" ] && exit 0
  fi
fi

root=$("$git_path" -C "$directory" rev-parse --show-toplevel) || exit 1
branch=$("$git_path" -C "$directory" symbolic-ref --quiet --short HEAD) || branch=
printf '%s\000%s' "$root" "$branch"
