use crate::error::Error;

const INIT_SNIPPET: &str = r#"export GV_ROOT="${GV_ROOT:-$HOME/.gv}"
export PATH="$GV_ROOT/shims:$GV_ROOT/bin:$PATH"
"#;

pub fn script(shell: &str) -> Result<String, Error> {
    match shell {
        "bash" | "zsh" => Ok(INIT_SNIPPET.to_string()),
        other => Err(Error::UnsupportedShell(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_and_zsh_print_the_same_snippet() {
        let bash = script("bash").unwrap();
        let zsh = script("zsh").unwrap();
        assert_eq!(bash, zsh);
        assert_eq!(
            bash,
            "export GV_ROOT=\"${GV_ROOT:-$HOME/.gv}\"\nexport PATH=\"$GV_ROOT/shims:$GV_ROOT/bin:$PATH\"\n"
        );
    }

    #[test]
    fn other_shells_are_rejected() {
        let err = script("fish").unwrap_err();
        assert!(err.to_string().contains("bash"));
        assert!(script("powershell").is_err());
    }
}
