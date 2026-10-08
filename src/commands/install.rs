use std::path::Path;

use crate::download::{self, InstallStatus};
use crate::error::Error;
use crate::platform::Platform;
use crate::resolve::{self, VersionQuery};
use crate::store;

pub async fn run(root: &Path, requested: &str) -> Result<(), Error> {
    let query = resolve::parse_user_spec(requested)?;
    if let VersionQuery::Exact(version) = &query {
        if store::tool_exists(root, &version.to_string(), "go") {
            println!("Go {version} 已安装");
            return Ok(());
        }
    }

    let platform = Platform::current()?;
    let mirror = store::mirror_url();
    let url = store::index_url();
    let mut loaded = download::load_index(root, &url, false).await?;
    let plan = match download::plan_install(&loaded.releases, requested, &platform, &mirror) {
        Ok(plan) => plan,
        Err(Error::VersionNotInIndex(_)) | Err(Error::NoArchive { .. }) if loaded.from_cache => {
            loaded = download::load_index(root, &url, true).await?;
            download::plan_install(&loaded.releases, requested, &platform, &mirror)?
        }
        Err(err) => return Err(err),
    };

    let version = plan.version.to_string();
    if store::tool_exists(root, &version, "go") {
        println!("Go {version} 已安装");
        return Ok(());
    }

    let bytes = download::http_get(&plan.url).await?;
    match download::install_verified_archive(
        &bytes,
        &plan.sha256,
        &store::versions_dir(root),
        &version,
    )? {
        InstallStatus::Installed => println!("已安装 Go {version}"),
        InstallStatus::AlreadyPresent => println!("Go {version} 已安装"),
    }
    Ok(())
}
