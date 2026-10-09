use crate::error::Error;

const BASH: &str = r#"# 把这段输出放进 shell 配置。gv 不会修改 ~/.bashrc。
_gv() {
  local cur cmd versions line
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  if [[ "${COMP_CWORD}" -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "install uninstall list list-remote use current which init completions clean shell" -- "${cur}") )
    return 0
  fi
  cmd="${COMP_WORDS[1]}"
  case "${cmd}" in
    install|uninstall|use|shell)
      if [[ "${cur}" == -* ]]; then
        case "${cmd}" in
          install) COMPREPLY=( $(compgen -W "--quiet" -- "${cur}") ) ;;
          uninstall) COMPREPLY=( $(compgen -W "--force" -- "${cur}") ) ;;
          use) COMPREPLY=( $(compgen -W "--global --unset" -- "${cur}") ) ;;
          shell) COMPREPLY=( $(compgen -W "--unset" -- "${cur}") ) ;;
        esac
        return 0
      fi
      versions=""
      while IFS= read -r line; do
        case "${line}" in
          \*\ *) versions="${versions} ${line#\* }" ;;
          "  "*) versions="${versions} ${line#  }" ;;
        esac
      done <<EOF
$(gv list 2>/dev/null)
EOF
      if [[ "${cmd}" == install ]]; then
        versions="${versions} latest"
      fi
      COMPREPLY=( $(compgen -W "${versions}" -- "${cur}") )
      ;;
    init|completions)
      COMPREPLY=( $(compgen -W "bash zsh" -- "${cur}") )
      ;;
    list)
      COMPREPLY=( $(compgen -W "--output text json" -- "${cur}") )
      ;;
    list-remote)
      COMPREPLY=( $(compgen -W "--all --refresh --output text json" -- "${cur}") )
      ;;
  esac
}
complete -F _gv gv
"#;

const ZSH: &str = r#"#compdef gv
# 把这段输出放进 shell 配置。gv 不会修改 ~/.zshrc。
_gv() {
  local -a commands versions
  commands=(install uninstall list list-remote use current which init completions clean shell)
  if (( CURRENT == 2 )); then
    compadd -a commands
    return
  fi
  local cmd="${words[2]}"
  case "${cmd}" in
    install|uninstall|use|shell)
      versions=(${(f)"$(gv list 2>/dev/null | sed -n 's/^[* ][[:space:]]*//p' | grep -E '^[0-9]')"})
      case "${cmd}" in
        install) versions+=(latest --quiet) ;;
        uninstall) versions+=(--force) ;;
        use) versions+=(--global --unset) ;;
        shell) versions+=(--unset) ;;
      esac
      compadd -a versions
      ;;
    init|completions)
      compadd bash zsh
      ;;
    list)
      compadd -- --output text json
      ;;
    list-remote)
      compadd -- --all --refresh --output text json
      ;;
  esac
}
if whence compdef >/dev/null 2>&1; then
  compdef _gv gv
fi
"#;

pub fn script(shell: &str) -> Result<String, Error> {
    match shell {
        "bash" => Ok(BASH.to_string()),
        "zsh" => Ok(ZSH.to_string()),
        other => Err(Error::UnsupportedShell(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_subcommand_and_installed_version_completion() {
        for shell in ["bash", "zsh"] {
            let text = script(shell).unwrap();
            for name in [
                "install",
                "uninstall",
                "list",
                "list-remote",
                "use",
                "current",
                "which",
                "init",
                "completions",
                "clean",
                "shell",
            ] {
                assert!(text.contains(name), "{shell} missing {name}");
            }
            assert!(text.contains("gv list"), "{shell}");
            assert!(text.contains("latest"), "{shell}");
            assert!(!text.contains("bashrc") || shell == "bash");
            assert!(!text.contains("zshrc") || shell == "zsh");
            assert!(!text.contains("cd "), "{shell}");
            assert!(!text.contains("builtin cd"), "{shell}");
        }
        let bash = script("bash").unwrap();
        assert!(bash.contains("不会修改 ~/.bashrc"));
        let zsh = script("zsh").unwrap();
        assert!(zsh.contains("不会修改 ~/.zshrc"));
        assert!(script("fish").unwrap_err().to_string().contains("bash"));
    }
}
