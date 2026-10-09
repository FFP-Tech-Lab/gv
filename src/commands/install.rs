use std::path::Path;
use std::time::SystemTime;

use crate::download::{self, IndexPolicy, InstallPlan, InstallStatus, LoadedIndex};
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
            println!("Go {version} is already installed");
            return Ok(());
        }
    }

    let platform = Platform::current()?;
    let mirror = store::mirror_url();
    let url = store::index_url();
    let client = download::http_client()?;
    let ui = crate::ui::Ui::detect(quiet);
    let mut loaded = load_for_install(&client, root, &url, policy_for(&query), &ui).await?;
    let plan = match download::plan_install(&loaded.releases, requested, &platform, &mirror) {
        Ok(plan) => plan,
        Err(err) if should_refresh(&err, &loaded) => {
            loaded = load_for_install(&client, root, &url, IndexPolicy::Refresh, &ui).await?;
            download::plan_install(&loaded.releases, requested, &platform, &mirror)?
        }
        Err(err) => return Err(err),
    };

    let version = plan.version.to_string();
    if store::tool_exists(root, &version, "go") {
        println!("Go {version} is already installed");
        return Ok(());
    }

    let versions_dir = store::versions_dir(root);
    let cache_dir = store::archive_cache_dir(root);
    let results = download::install_plans(
        &client,
        vec![plan],
        download::transfer_ui_for(quiet),
        &versions_dir,
        &cache_dir,
    )
    .await;
    match results.into_iter().next() {
        Some(Ok((_, InstallStatus::Installed))) => {
            println!("Installed Go {version}");
            hint_if_unused(root, &version);
        }
        Some(Ok((_, InstallStatus::AlreadyPresent))) => {
            println!("Go {version} is already installed")
        }
        Some(Err((_, err))) => return Err(err),
        None => return Err(Error::Failed("no install result".into())),
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
                    println!("Go {name} is already installed");
                } else {
                    pending.push(spec.clone());
                }
            }
            Ok(VersionQuery::Minor { .. } | VersionQuery::Latest) => pending.push(spec.clone()),
            Err(err) => failures.push(format!("Failed to install Go {spec}: {err}")),
        }
    }

    if !pending.is_empty() {
        match install_pending(root, &pending, quiet).await {
            Ok(more) => failures.extend(more),
            Err(err) => {
                let text = err.to_string();
                for spec in &pending {
                    failures.push(format!("Failed to install Go {spec}: {text}"));
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
    let client = download::http_client()?;
    let policy = if pending.iter().any(|spec| {
        matches!(
            resolve::parse_install_spec(spec),
            Ok(VersionQuery::Minor { .. } | VersionQuery::Latest)
        )
    }) {
        IndexPolicy::RefreshIfStale
    } else {
        IndexPolicy::AllowStale
    };
    let ui = crate::ui::Ui::detect(quiet);
    let loaded = load_for_install(&client, root, &url, policy, &ui).await?;
    let mut failures = Vec::new();
    let mut plans = Vec::new();
    let mut seen: Vec<Version> = Vec::new();
    let mut refresh_needed = Vec::new();

    for spec in pending {
        match download::plan_install(&loaded.releases, spec, &platform, &mirror) {
            Ok(plan) => queue_plan(&mut plans, &mut seen, root, plan),
            Err(err) if should_refresh(&err, &loaded) => {
                refresh_needed.push(spec.clone());
            }
            Err(err) => failures.push(format!("Failed to install Go {spec}: {err}")),
        }
    }

    if !refresh_needed.is_empty() {
        match load_for_install(&client, root, &url, IndexPolicy::Refresh, &ui).await {
            Ok(fresh) => {
                for spec in refresh_needed {
                    match download::plan_install(&fresh.releases, &spec, &platform, &mirror) {
                        Ok(plan) => queue_plan(&mut plans, &mut seen, root, plan),
                        Err(err) => failures.push(format!("Failed to install Go {spec}: {err}")),
                    }
                }
            }
            Err(err) => {
                let text = err.to_string();
                for spec in refresh_needed {
                    failures.push(format!("Failed to install Go {spec}: {text}"));
                }
            }
        }
    }

    let versions_dir = store::versions_dir(root);
    let cache_dir = store::archive_cache_dir(root);
    for item in download::install_plans(
        &client,
        plans,
        download::transfer_ui_for(quiet),
        &versions_dir,
        &cache_dir,
    )
    .await
    {
        match item {
            Ok((version, InstallStatus::Installed)) => println!("Installed Go {version}"),
            Ok((version, InstallStatus::AlreadyPresent)) => {
                println!("Go {version} is already installed");
            }
            Err((version, err)) => failures.push(format!("Failed to install Go {version}: {err}")),
        }
    }
    Ok(failures)
}

fn policy_for(query: &VersionQuery) -> IndexPolicy {
    match query {
        VersionQuery::Exact(_) => IndexPolicy::AllowStale,
        VersionQuery::Minor { .. } | VersionQuery::Latest => IndexPolicy::RefreshIfStale,
    }
}

async fn load_for_install(
    client: &reqwest::Client,
    root: &Path,
    url: &str,
    policy: IndexPolicy,
    ui: &crate::ui::Ui,
) -> Result<LoadedIndex, Error> {
    let loaded =
        download::load_index_with_ui(client, root, url, policy, SystemTime::now(), ui).await?;
    if let Some(warning) = &loaded.warning {
        eprintln!("gv: {warning}");
    }
    Ok(loaded)
}

fn should_refresh(err: &Error, loaded: &LoadedIndex) -> bool {
    matches!(err, Error::VersionNotInIndex(_) | Error::NoArchive { .. })
        && loaded.from_cache
        && !loaded.refresh_attempted
}

fn hint_if_unused(root: &Path, version: &str) {
    let Ok(cwd) = std::env::current_dir() else {
        eprintln!("gv: To use it in this directory, run gv use {version}");
        return;
    };
    let gv_version = std::env::var("GV_VERSION").ok();
    let active = resolve::resolve(&cwd, root, gv_version.as_deref()).ok();
    if active.is_some_and(|resolved| resolved.version.to_string() == version) {
        return;
    }
    eprintln!("gv: To use it in this directory, run gv use {version}");
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
        println!("Go {name} is already installed");
        return;
    }
    plans.push(plan);
}
