use std::path::Path;

use crate::download::{self, InstallPlan, InstallStatus};
use crate::error::Error;
use crate::platform::Platform;
use crate::resolve::{self, Version, VersionQuery};
use crate::store;

pub async fn run(root: &Path, requested: &[String], quiet: bool) -> Result<(), Error> {
    if requested.len() == 1 {
        return install_one(root, &requested[0], quiet).await;
    }
    install_many(root, requested, quiet).await
}

async fn install_one(root: &Path, requested: &str, quiet: bool) -> Result<(), Error> {
    let query = resolve::parse_install_spec(requested)?;
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

    let versions_dir = store::versions_dir(root);
    let cache_dir = store::archive_cache_dir(root);
    let results = download::install_plans(
        vec![plan],
        download::download_ui_for(quiet),
        &versions_dir,
        &cache_dir,
    )
    .await;
    match results.into_iter().next() {
        Some(Ok((_, InstallStatus::Installed))) => println!("已安装 Go {version}"),
        Some(Ok((_, InstallStatus::AlreadyPresent))) => println!("Go {version} 已安装"),
        Some(Err((_, err))) => return Err(err),
        None => return Err(Error::Failed("没有安装结果".into())),
    }
    Ok(())
}

async fn install_many(root: &Path, requested: &[String], quiet: bool) -> Result<(), Error> {
    let mut failures = Vec::new();
    let mut pending = Vec::new();
    for spec in requested {
        match resolve::parse_install_spec(spec) {
            Ok(VersionQuery::Exact(version)) => {
                let name = version.to_string();
                if store::tool_exists(root, &name, "go") {
                    println!("Go {name} 已安装");
                } else {
                    pending.push(spec.clone());
                }
            }
            Ok(VersionQuery::Minor { .. } | VersionQuery::Latest) => pending.push(spec.clone()),
            Err(err) => failures.push(format!("Go {spec} 安装失败：{err}")),
        }
    }

    if !pending.is_empty() {
        match install_pending(root, &pending, quiet).await {
            Ok(more) => failures.extend(more),
            Err(err) => {
                let text = err.to_string();
                for spec in &pending {
                    failures.push(format!("Go {spec} 安装失败：{text}"));
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Failed(failures.join("\n")))
    }
}

async fn install_pending(
    root: &Path,
    pending: &[String],
    quiet: bool,
) -> Result<Vec<String>, Error> {
    let platform = Platform::current()?;
    let mirror = store::mirror_url();
    let url = store::index_url();
    let loaded = download::load_index(root, &url, false).await?;
    let mut failures = Vec::new();
    let mut plans = Vec::new();
    let mut seen: Vec<Version> = Vec::new();
    let mut refresh_needed = Vec::new();

    for spec in pending {
        match download::plan_install(&loaded.releases, spec, &platform, &mirror) {
            Ok(plan) => queue_plan(&mut plans, &mut seen, root, plan),
            Err(Error::VersionNotInIndex(_)) | Err(Error::NoArchive { .. })
                if loaded.from_cache =>
            {
                refresh_needed.push(spec.clone());
            }
            Err(err) => failures.push(format!("Go {spec} 安装失败：{err}")),
        }
    }

    if !refresh_needed.is_empty() {
        match download::load_index(root, &url, true).await {
            Ok(fresh) => {
                for spec in refresh_needed {
                    match download::plan_install(&fresh.releases, &spec, &platform, &mirror) {
                        Ok(plan) => queue_plan(&mut plans, &mut seen, root, plan),
                        Err(err) => failures.push(format!("Go {spec} 安装失败：{err}")),
                    }
                }
            }
            Err(err) => {
                let text = err.to_string();
                for spec in refresh_needed {
                    failures.push(format!("Go {spec} 安装失败：{text}"));
                }
            }
        }
    }

    let versions_dir = store::versions_dir(root);
    let cache_dir = store::archive_cache_dir(root);
    for item in download::install_plans(
        plans,
        download::download_ui_for(quiet),
        &versions_dir,
        &cache_dir,
    )
    .await
    {
        match item {
            Ok((version, InstallStatus::Installed)) => println!("已安装 Go {version}"),
            Ok((version, InstallStatus::AlreadyPresent)) => println!("Go {version} 已安装"),
            Err((version, err)) => failures.push(format!("Go {version} 安装失败：{err}")),
        }
    }
    Ok(failures)
}

fn queue_plan(
    plans: &mut Vec<InstallPlan>,
    seen: &mut Vec<Version>,
    root: &Path,
    plan: InstallPlan,
) {
    if seen.iter().any(|version| version == &plan.version) {
        return;
    }
    seen.push(plan.version.clone());
    let name = plan.version.to_string();
    if store::tool_exists(root, &name, "go") {
        println!("Go {name} 已安装");
        return;
    }
    plans.push(plan);
}
