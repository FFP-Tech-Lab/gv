use crate::error::Error;
use crate::resolve;

pub fn script(requested: Option<&str>) -> Result<String, Error> {
    let Some(requested) = requested else {
        return Ok("unset GV_VERSION\n".to_string());
    };
    let query = resolve::parse_user_spec(requested)?;
    Ok(format!("export GV_VERSION={}\n", query.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_one_export_or_unset_line() {
        assert_eq!(
            script(Some("1.23.4")).unwrap(),
            "export GV_VERSION=1.23.4\n"
        );
        assert_eq!(
            script(Some("go1.23.4")).unwrap(),
            "export GV_VERSION=1.23.4\n"
        );
        assert_eq!(script(Some("1.23")).unwrap(), "export GV_VERSION=1.23\n");
        assert_eq!(script(None).unwrap(), "unset GV_VERSION\n");
        assert!(!script(Some("1.23.4")).unwrap().contains("cd "));
        assert!(script(Some("latest")).is_err());
    }
}
