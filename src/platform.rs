use crate::error::Error;

/// Operating system and architecture names used by Go release archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    pub fn detect(os: &str, arch: &str) -> Result<Self, Error> {
        let go_os = match os {
            "linux" => "linux",
            "macos" | "darwin" => "darwin",
            _ => return Err(Error::UnsupportedPlatform(os.to_string(), arch.to_string())),
        };
        let go_arch = match arch {
            "x86_64" | "amd64" => "amd64",
            "aarch64" | "arm64" => "arm64",
            _ => return Err(Error::UnsupportedPlatform(os.to_string(), arch.to_string())),
        };
        Ok(Self {
            os: go_os.to_string(),
            arch: go_arch.to_string(),
        })
    }

    pub fn current() -> Result<Self, Error> {
        Self::detect(std::env::consts::OS, std::env::consts::ARCH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_linux_and_macos() {
        assert_eq!(
            Platform::detect("linux", "x86_64").unwrap(),
            Platform {
                os: "linux".into(),
                arch: "amd64".into(),
            }
        );
        assert_eq!(
            Platform::detect("macos", "aarch64").unwrap(),
            Platform {
                os: "darwin".into(),
                arch: "arm64".into(),
            }
        );
        assert_eq!(Platform::detect("darwin", "arm64").unwrap().arch, "arm64");
    }

    #[test]
    fn rejects_windows_and_unknown_arch() {
        let err = Platform::detect("windows", "x86_64").unwrap_err();
        assert!(err.to_string().contains("Unsupported platform"));
        assert!(Platform::detect("linux", "powerpc64").is_err());
    }

    #[test]
    fn current_host_is_supported() {
        let platform = Platform::current().unwrap();
        assert!(platform.os == "linux" || platform.os == "darwin");
        assert!(platform.arch == "amd64" || platform.arch == "arm64");
    }
}
