use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    process::Command,
    time::{Duration, timeout},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

pub async fn run_readonly_command(
    program: &str,
    args: &[String],
    cwd: impl AsRef<Path>,
    environment: &BTreeMap<String, String>,
    timeout_seconds: u64,
) -> Result<CommandOutput> {
    if program.trim().is_empty() {
        bail!("devtool program cannot be empty");
    }
    let cwd = cwd
        .as_ref()
        .canonicalize()
        .context("devtool cwd does not exist")?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();

    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    for (key, value) in environment {
        validate_env_key(key)?;
        command.env(key, value);
    }

    let mut child = command.spawn().context("spawn developer tool")?;
    match timeout(
        Duration::from_secs(timeout_seconds.max(1)),
        child.wait_with_output(),
    )
    .await
    {
        Ok(output) => {
            let output = output?;
            Ok(CommandOutput {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            })
        }
        Err(_) => {
            let _ = child.kill().await;
            Ok(CommandOutput {
                exit_code: -1,
                stdout: String::new(),
                stderr: "command timed out".into(),
                timed_out: true,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseKind {
    Postgres,
    MySql,
    Sqlite,
    Redis,
    Mongo,
    ClickHouse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConnection {
    pub kind: DatabaseKind,
    pub database: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

pub async fn introspect_database(
    connection: &DatabaseConnection,
    cwd: impl AsRef<Path>,
) -> Result<Value> {
    let (program, args) = match connection.kind {
        DatabaseKind::Postgres => (
            "psql",
            vec![
                "-X".into(),
                "--no-psqlrc".into(),
                "--set".into(),
                "ON_ERROR_STOP=1".into(),
                "--dbname".into(),
                connection.database.clone(),
                "--csv".into(),
                "--command".into(),
                "SELECT table_schema,table_name,column_name,data_type,is_nullable FROM information_schema.columns WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY table_schema,table_name,ordinal_position".into(),
            ],
        ),
        DatabaseKind::MySql => (
            "mysql",
            vec![
                "--batch".into(),
                "--raw".into(),
                connection.database.clone(),
                "--execute".into(),
                "SELECT TABLE_SCHEMA,TABLE_NAME,COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME,ORDINAL_POSITION".into(),
            ],
        ),
        DatabaseKind::Sqlite => (
            "sqlite3",
            vec![
                "-json".into(),
                connection.database.clone(),
                r#"SELECT m.name AS table_name,p.name AS column_name,p.type AS data_type,p."notnull" AS not_null,p.pk AS primary_key FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type='table' ORDER BY m.name,p.cid"#.into(),
            ],
        ),
        DatabaseKind::Redis => (
            "redis-cli",
            vec![
                "-u".into(),
                connection.database.clone(),
                "--json".into(),
                "INFO".into(),
                "ALL".into(),
            ],
        ),
        DatabaseKind::Mongo => (
            "mongosh",
            vec![
                connection.database.clone(),
                "--quiet".into(),
                "--eval".into(),
                "JSON.stringify(db.getCollectionNames().map(n=>({name:n,indexes:db.getCollection(n).getIndexes()})))".into(),
            ],
        ),
        DatabaseKind::ClickHouse => (
            "clickhouse-client",
            vec![
                "--host".into(),
                connection.database.clone(),
                "--format".into(),
                "JSON".into(),
                "--query".into(),
                "SELECT database,table,name,type FROM system.columns WHERE database NOT IN ('system','INFORMATION_SCHEMA','information_schema') ORDER BY database,table,position".into(),
            ],
        ),
    };

    let output = run_readonly_command(program, &args, cwd, &connection.environment, 60).await?;
    if output.exit_code != 0 {
        bail!("database introspection failed: {}", output.stderr.trim());
    }

    Ok(serde_json::json!({
        "kind": connection.kind,
        "raw": output.stdout
    }))
}

pub async fn docker_inventory(cwd: impl AsRef<Path>) -> Result<Value> {
    let output = run_readonly_command(
        "docker",
        &[
            "ps".into(),
            "-a".into(),
            "--format".into(),
            "{{json .}}".into(),
        ],
        cwd,
        &BTreeMap::new(),
        30,
    )
    .await?;
    require_success("docker ps", output)
}

pub async fn docker_inspect(cwd: impl AsRef<Path>, object: &str) -> Result<Value> {
    validate_resource_name(object)?;
    let output = run_readonly_command(
        "docker",
        &["inspect".into(), object.into()],
        cwd,
        &BTreeMap::new(),
        30,
    )
    .await?;
    require_success("docker inspect", output)
}

pub async fn kubernetes_inventory(cwd: impl AsRef<Path>, namespace: Option<&str>) -> Result<Value> {
    let mut args = vec![
        "get".into(),
        "pods,deployments,statefulsets,services,ingresses,jobs".into(),
        "-o".into(),
        "json".into(),
    ];
    if let Some(namespace) = namespace {
        validate_resource_name(namespace)?;
        args.extend(["-n".into(), namespace.into()]);
    } else {
        args.push("-A".into());
    }

    let output = run_readonly_command("kubectl", &args, cwd, &BTreeMap::new(), 60).await?;
    require_success("kubectl get", output)
}

pub async fn terraform_plan_json(
    cwd: impl AsRef<Path>,
    variable_files: &[PathBuf],
) -> Result<Value> {
    let cwd = cwd.as_ref();
    let plan_path = cwd.join(".openforge").join("terraform-plan.tfplan");
    if let Some(parent) = plan_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut args = vec![
        "plan".into(),
        "-input=false".into(),
        "-lock=false".into(),
        "-out".into(),
        plan_path.display().to_string(),
    ];
    for path in variable_files {
        let canonical = cwd.join(path).canonicalize()?;
        if !canonical.starts_with(cwd.canonicalize()?) {
            bail!("terraform variable file escapes workspace");
        }
        args.push(format!("-var-file={}", canonical.display()));
    }

    let plan = run_readonly_command("terraform", &args, cwd, &BTreeMap::new(), 600).await?;
    if plan.exit_code != 0 {
        bail!("terraform plan failed: {}", plan.stderr.trim());
    }

    let shown = run_readonly_command(
        "terraform",
        &[
            "show".into(),
            "-json".into(),
            plan_path.display().to_string(),
        ],
        cwd,
        &BTreeMap::new(),
        120,
    )
    .await?;
    if shown.exit_code != 0 {
        bail!("terraform show failed: {}", shown.stderr.trim());
    }
    Ok(serde_json::from_str(&shown.stdout).context("parse terraform plan JSON")?)
}

fn require_success(name: &str, output: CommandOutput) -> Result<Value> {
    if output.exit_code != 0 || output.timed_out {
        bail!("{name} failed: {}", output.stderr.trim());
    }
    if output.stdout.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&output.stdout)
        .or_else(|_| {
            let values = output
                .stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<serde_json::Result<Vec<Value>>>()?;
            Ok(Value::Array(values))
        })
        .context("parse developer tool JSON output")
}

fn validate_resource_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.:/".contains(character))
    {
        bail!("invalid resource identifier");
    }
    Ok(())
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid devtool environment variable name {key:?}");
    }
    Ok(())
}
