use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use openforge_protocol::SandboxSecurityProfile;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{timeout, Duration},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub cpus: f32,
    pub memory_mb: u64,
    pub pids_limit: u32,
    pub network_enabled: bool,
    pub disk_mb: u64,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub security: SandboxSecurityProfile,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            cpus: 2.0,
            memory_mb: 4096,
            pids_limit: 256,
            network_enabled: false,
            disk_mb: 20_480,
            max_stdout_bytes: 8 * 1024 * 1024,
            max_stderr_bytes: 8 * 1024 * 1024,
            environment: BTreeMap::new(),
            security: SandboxSecurityProfile::default(),
        }
    }
}

impl SandboxPolicy {
    pub fn validate(&self) -> Result<()> {
        if !self.cpus.is_finite() || self.cpus <= 0.0 {
            bail!("sandbox cpus must be a positive finite number");
        }
        if self.memory_mb < 128 {
            bail!("sandbox memory_mb must be at least 128");
        }
        if self.pids_limit == 0 {
            bail!("sandbox pids_limit must be greater than zero");
        }
        if self.disk_mb < 64 {
            bail!("sandbox disk_mb must be at least 64");
        }
        if self.max_stdout_bytes == 0 || self.max_stderr_bytes == 0 {
            bail!("sandbox output limits must be greater than zero");
        }
        for key in self.environment.keys() {
            validate_env_key(key)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SandboxLease {
    pub id: Uuid,
    pub backend: String,
    pub workspace: PathBuf,
    pub policy: SandboxPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[async_trait]
pub trait SandboxBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn create(&self, workspace: &Path, policy: SandboxPolicy) -> Result<SandboxLease>;
    async fn exec(&self, lease: &SandboxLease, request: ExecRequest) -> Result<ExecResult>;
    async fn destroy(&self, lease: SandboxLease) -> Result<()>;
}

pub struct LocalProcessBackend;

#[async_trait]
impl SandboxBackend for LocalProcessBackend {
    fn name(&self) -> &str {
        "local-process"
    }

    async fn create(&self, workspace: &Path, policy: SandboxPolicy) -> Result<SandboxLease> {
        policy.validate()?;
        let canonical = workspace.canonicalize().context("workspace does not exist")?;
        Ok(SandboxLease {
            id: Uuid::new_v4(),
            backend: self.name().into(),
            workspace: canonical,
            policy,
        })
    }

    async fn exec(&self, lease: &SandboxLease, request: ExecRequest) -> Result<ExecResult> {
        validate_exec_request(&request)?;
        let cwd = safe_cwd(&lease.workspace, &request.cwd)?;
        let mut command = Command::new(&request.argv[0]);
        command
            .args(&request.argv[1..])
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.env_clear();

        for (key, value) in std::env::vars().filter(|(key, _)| {
            ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR"].contains(&key.as_str())
        }) {
            command.env(key, value);
        }
        for (key, value) in &lease.policy.environment {
            command.env(key, value);
        }
        for (key, value) in &request.environment {
            validate_env_key(key)?;
            command.env(key, value);
        }

        run_bounded(
            command,
            request.timeout_seconds,
            lease.policy.max_stdout_bytes,
            lease.policy.max_stderr_bytes,
        )
        .await
    }

    async fn destroy(&self, _lease: SandboxLease) -> Result<()> {
        Ok(())
    }
}

pub struct DockerBackend {
    pub image: String,
}

#[async_trait]
impl SandboxBackend for DockerBackend {
    fn name(&self) -> &str {
        "docker"
    }

    async fn create(&self, workspace: &Path, policy: SandboxPolicy) -> Result<SandboxLease> {
        policy.validate()?;
        let canonical = workspace.canonicalize().context("workspace does not exist")?;
        let result = host_command(
            "docker",
            &["version", "--format", "{{.Server.Version}}"],
            Duration::from_secs(15),
        )
        .await
        .context("docker executable or daemon unavailable")?;
        if result.exit_code != 0 {
            bail!("docker daemon unavailable: {}", result.stderr);
        }

        Ok(SandboxLease {
            id: Uuid::new_v4(),
            backend: self.name().into(),
            workspace: canonical,
            policy,
        })
    }

    async fn exec(&self, lease: &SandboxLease, request: ExecRequest) -> Result<ExecResult> {
        validate_exec_request(&request)?;
        let relative_cwd = normalized_relative(&request.cwd)?;
        let name = format!(
            "openforge-{}-{}",
            lease.id.simple(),
            Uuid::new_v4().simple()
        );
        let mount = format!("{}:/workspace:rw", lease.workspace.display());
        let memory = format!("{}m", lease.policy.memory_mb);
        let memory_swap = memory.clone();
        let tmp_size = (lease.policy.memory_mb / 4).clamp(64, 1024);
        let mut arguments = vec![
            "run".to_string(),
            "--rm".into(),
            "--init".into(),
            "--name".into(),
            name.clone(),
            "--cpus".into(),
            lease.policy.cpus.to_string(),
            "--memory".into(),
            memory,
            "--memory-swap".into(),
            memory_swap,
            "--pids-limit".into(),
            lease.policy.pids_limit.to_string(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--user".into(),
            "10001:10001".into(),
            "--tmpfs".into(),
            format!("/tmp:rw,nosuid,nodev,size={}m", tmp_size),
            "-v".into(),
            mount,
            "-w".into(),
            if relative_cwd.as_os_str().is_empty() {
                "/workspace".into()
            } else {
                format!("/workspace/{}", relative_cwd.display())
            },
        ];

        if lease.policy.security.read_only_root {
            arguments.push("--read-only".into());
        }

        if lease.policy.network_enabled {
            arguments.push("--network".into());
            arguments.push("bridge".into());
        } else {
            arguments.push("--network".into());
            arguments.push("none".into());
        }

        for (key, value) in lease
            .policy
            .environment
            .iter()
            .chain(request.environment.iter())
        {
            validate_env_key(key)?;
            arguments.push("-e".into());
            arguments.push(format!("{key}={value}"));
        }

        arguments.push(self.image.clone());
        arguments.extend(request.argv);

        let mut command = Command::new("docker");
        command
            .args(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let duration = request.timeout_seconds.max(1);
        match run_bounded(
            command,
            duration,
            lease.policy.max_stdout_bytes,
            lease.policy.max_stderr_bytes,
        )
        .await
        {
            Ok(result) => {
                if result.timed_out {
                    let _ = host_command(
                        "docker",
                        &["rm", "-f", &name],
                        Duration::from_secs(10),
                    )
                    .await;
                }
                Ok(result)
            }
            Err(error) => {
                let _ = host_command(
                    "docker",
                    &["rm", "-f", &name],
                    Duration::from_secs(10),
                )
                .await;
                Err(error)
            }
        }
    }

    async fn destroy(&self, _lease: SandboxLease) -> Result<()> {
        Ok(())
    }
}


pub struct KubernetesBackend {
    pub image: String,
    pub namespace: String,
    pub wait_seconds: u64,
}

impl KubernetesBackend {
    fn pod_name(&self, lease_id: Uuid) -> String {
        format!("openforge-{}", lease_id.simple())
    }

    async fn cleanup_pod(&self, pod_name: &str) {
        let _ = host_command_owned(
            "kubectl",
            &[
                "-n".into(),
                self.namespace.clone(),
                "delete".into(),
                "networkpolicy".into(),
                pod_name.into(),
                "--ignore-not-found=true".into(),
                "--wait=false".into(),
            ],
            Duration::from_secs(20),
        )
        .await;
        let _ = host_command_owned(
            "kubectl",
            &[
                "-n".into(),
                self.namespace.clone(),
                "delete".into(),
                "pod".into(),
                pod_name.into(),
                "--ignore-not-found=true".into(),
                "--wait=false".into(),
            ],
            Duration::from_secs(20),
        )
        .await;
    }
}

#[async_trait]
impl SandboxBackend for KubernetesBackend {
    fn name(&self) -> &str {
        "kubernetes"
    }

    async fn create(&self, workspace: &Path, policy: SandboxPolicy) -> Result<SandboxLease> {
        policy.validate()?;
        validate_kubernetes_name(&self.namespace, "namespace")?;
        if self.image.trim().is_empty() {
            bail!("Kubernetes runner image cannot be empty");
        }

        let canonical = workspace.canonicalize().context("workspace does not exist")?;
        let version = host_command_owned(
            "kubectl",
            &["version".into(), "--client=true".into(), "-o".into(), "json".into()],
            Duration::from_secs(15),
        )
        .await
        .context("kubectl executable unavailable")?;
        if version.exit_code != 0 {
            bail!("kubectl unavailable: {}", version.stderr);
        }

        let lease_id = Uuid::new_v4();
        let pod_name = self.pod_name(lease_id);
        let cpu_limit = policy.cpus.to_string();
        let memory_limit = format!("{}Mi", policy.memory_mb);
        let disk_limit = format!("{}Mi", policy.disk_mb);
        let tmp_size = format!("{}Mi", (policy.memory_mb / 4).clamp(64, 1024));

        let manifest = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": pod_name,
                "namespace": self.namespace,
                "labels": {
                    "app.kubernetes.io/name": "openforge-runner",
                    "openforge.dev/sandbox": pod_name
                }
            },
            "spec": {
                "automountServiceAccountToken": false,
                "restartPolicy": "Never",
                "securityContext": {
                    "runAsNonRoot": true,
                    "runAsUser": 10001,
                    "runAsGroup": 10001,
                    "fsGroup": 10001,
                    "seccompProfile": {"type": "RuntimeDefault"}
                },
                "containers": [{
                    "name": "runner",
                    "image": self.image,
                    "imagePullPolicy": "IfNotPresent",
                    "workingDir": "/workspace",
                    "command": ["bash", "-lc", "sleep infinity"],
                    "resources": {
                        "requests": {
                            "cpu": cpu_limit,
                            "memory": memory_limit
                        },
                        "limits": {
                            "cpu": cpu_limit,
                            "memory": memory_limit
                        }
                    },
                    "securityContext": {
                        "allowPrivilegeEscalation": false,
                        "readOnlyRootFilesystem": policy.security.read_only_root,
                        "capabilities": {"drop": ["ALL"]}
                    },
                    "volumeMounts": [
                        {"name": "workspace", "mountPath": "/workspace"},
                        {"name": "tmp", "mountPath": "/tmp"}
                    ]
                }],
                "volumes": [
                    {"name": "workspace", "emptyDir": {"sizeLimit": disk_limit}},
                    {"name": "tmp", "emptyDir": {"sizeLimit": tmp_size}}
                ]
            }
        });

        let manifest_path = std::env::temp_dir().join(format!("{pod_name}.json"));
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        let apply = host_command_owned(
            "kubectl",
            &[
                "-n".into(),
                self.namespace.clone(),
                "apply".into(),
                "-f".into(),
                manifest_path.display().to_string(),
            ],
            Duration::from_secs(30),
        )
        .await;
        let _ = std::fs::remove_file(&manifest_path);
        let apply = apply?;
        if apply.exit_code != 0 {
            self.cleanup_pod(&pod_name).await;
            bail!("failed to create Kubernetes sandbox: {}", apply.stderr);
        }

        if !policy.network_enabled {
            let network_policy = serde_json::json!({
                "apiVersion": "networking.k8s.io/v1",
                "kind": "NetworkPolicy",
                "metadata": {
                    "name": pod_name,
                    "namespace": self.namespace
                },
                "spec": {
                    "podSelector": {
                        "matchLabels": {"openforge.dev/sandbox": pod_name}
                    },
                    "policyTypes": ["Ingress", "Egress"],
                    "ingress": [],
                    "egress": []
                }
            });
            let policy_path = std::env::temp_dir().join(format!("{pod_name}.network.json"));
            std::fs::write(&policy_path, serde_json::to_vec(&network_policy)?)?;
            let applied = host_command_owned(
                "kubectl",
                &[
                    "-n".into(),
                    self.namespace.clone(),
                    "apply".into(),
                    "-f".into(),
                    policy_path.display().to_string(),
                ],
                Duration::from_secs(30),
            )
            .await;
            let _ = std::fs::remove_file(&policy_path);
            match applied {
                Ok(result) if result.exit_code == 0 => {}
                Ok(result) => {
                    self.cleanup_pod(&pod_name).await;
                    bail!("failed to apply Kubernetes network isolation: {}", result.stderr);
                }
                Err(error) => {
                    self.cleanup_pod(&pod_name).await;
                    return Err(error.context("apply Kubernetes network isolation"));
                }
            }
        }

        let wait = host_command_owned(
            "kubectl",
            &[
                "-n".into(),
                self.namespace.clone(),
                "wait".into(),
                "--for=condition=Ready".into(),
                format!("pod/{pod_name}"),
                format!("--timeout={}s", self.wait_seconds.clamp(10, 600)),
            ],
            Duration::from_secs(self.wait_seconds.clamp(10, 600) + 10),
        )
        .await?;
        if wait.exit_code != 0 {
            self.cleanup_pod(&pod_name).await;
            bail!("Kubernetes sandbox did not become ready: {}", wait.stderr);
        }

        let copy = host_command_owned(
            "kubectl",
            &[
                "-n".into(),
                self.namespace.clone(),
                "cp".into(),
                format!("{}/.", canonical.display()),
                format!("{pod_name}:/workspace"),
            ],
            Duration::from_secs(120),
        )
        .await?;
        if copy.exit_code != 0 {
            self.cleanup_pod(&pod_name).await;
            bail!("failed to upload workspace to Kubernetes sandbox: {}", copy.stderr);
        }

        Ok(SandboxLease {
            id: lease_id,
            backend: self.name().into(),
            workspace: canonical,
            policy,
        })
    }

    async fn exec(&self, lease: &SandboxLease, request: ExecRequest) -> Result<ExecResult> {
        validate_exec_request(&request)?;
        let relative_cwd = normalized_relative(&request.cwd)?;
        let pod_name = self.pod_name(lease.id);
        let remote_cwd = if relative_cwd.as_os_str().is_empty() {
            "/workspace".to_string()
        } else {
            format!("/workspace/{}", relative_cwd.display())
        };

        let mut environment = lease.policy.environment.clone();
        environment.extend(request.environment.clone());

        let mut arguments = vec![
            "-n".to_string(),
            self.namespace.clone(),
            "exec".into(),
            pod_name,
            "--".into(),
            "env".into(),
        ];
        for (key, value) in environment {
            validate_env_key(&key)?;
            arguments.push(format!("{key}={value}"));
        }
        arguments.extend([
            "bash".into(),
            "-lc".into(),
            "cd \"$1\" && shift && exec \"$@\"".into(),
            "openforge".into(),
            remote_cwd,
        ]);
        arguments.extend(request.argv);

        let mut command = Command::new("kubectl");
        command
            .args(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        run_bounded(
            command,
            request.timeout_seconds.max(1),
            lease.policy.max_stdout_bytes,
            lease.policy.max_stderr_bytes,
        )
        .await
    }

    async fn destroy(&self, lease: SandboxLease) -> Result<()> {
        let pod_name = self.pod_name(lease.id);
        let copy_back = host_command_owned(
            "kubectl",
            &[
                "-n".into(),
                self.namespace.clone(),
                "cp".into(),
                format!("{pod_name}:/workspace/."),
                lease.workspace.display().to_string(),
            ],
            Duration::from_secs(120),
        )
        .await;
        self.cleanup_pod(&pod_name).await;

        match copy_back {
            Ok(result) if result.exit_code == 0 => Ok(()),
            Ok(result) => bail!("failed to download Kubernetes workspace: {}", result.stderr),
            Err(error) => Err(error.context("download Kubernetes workspace")),
        }
    }
}

fn validate_kubernetes_name(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 63
        || value.starts_with('-')
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        bail!("invalid Kubernetes {kind} {value:?}");
    }
    Ok(())
}

async fn run_bounded(
    mut command: Command,
    timeout_seconds: u64,
    max_stdout: u64,
    max_stderr: u64,
) -> Result<ExecResult> {
    let mut child = command.spawn().context("spawn sandbox process")?;
    let stdout = child.stdout.take().context("sandbox stdout unavailable")?;
    let stderr = child.stderr.take().context("sandbox stderr unavailable")?;

    let stdout_task = tokio::spawn(read_limited(stdout, max_stdout));
    let stderr_task = tokio::spawn(read_limited(stderr, max_stderr));

    let duration = Duration::from_secs(timeout_seconds.max(1));
    let (exit_code, timed_out) = match timeout(duration, child.wait()).await {
        Ok(status) => (status?.code().unwrap_or(-1), false),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (-1, true)
        }
    };

    let (stdout_bytes, stdout_truncated) = stdout_task.await??;
    let (stderr_bytes, stderr_truncated) = stderr_task.await??;

    Ok(ExecResult {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out,
        stdout_truncated,
        stderr_truncated,
    })
}

async fn read_limited<R>(reader: R, maximum: u64) -> Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut limited = reader.take(maximum.saturating_add(1));
    let mut bytes = Vec::with_capacity(maximum.min(1024 * 1024) as usize);
    limited.read_to_end(&mut bytes).await?;
    let truncated = bytes.len() as u64 > maximum;
    if truncated {
        bytes.truncate(maximum as usize);
    }
    Ok((bytes, truncated))
}

async fn host_command(program: &str, args: &[&str], duration: Duration) -> Result<ExecResult> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    run_bounded(command, duration.as_secs().max(1), 1024 * 1024, 1024 * 1024).await
}


async fn host_command_owned(
    program: &str,
    args: &[String],
    duration: Duration,
) -> Result<ExecResult> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    run_bounded(
        command,
        duration.as_secs().max(1),
        4 * 1024 * 1024,
        4 * 1024 * 1024,
    )
    .await
}

fn validate_exec_request(request: &ExecRequest) -> Result<()> {
    if request.argv.is_empty() {
        bail!("empty argv");
    }
    if request.argv[0].contains('\0') || request.cwd.contains('\0') {
        bail!("execution request contains NUL byte");
    }
    normalized_relative(&request.cwd)?;
    for key in request.environment.keys() {
        validate_env_key(key)?;
    }
    Ok(())
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || key.contains('\0')
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("invalid environment key {key:?}");
    }
    Ok(())
}

fn normalized_relative(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("cwd must remain inside workspace");
    }
    Ok(path.to_path_buf())
}

fn safe_cwd(workspace: &Path, value: &str) -> Result<PathBuf> {
    let relative = normalized_relative(value)?;
    let candidate = workspace.join(relative);
    let canonical = candidate.canonicalize().context("invalid cwd")?;
    if !canonical.starts_with(workspace) {
        bail!("cwd escapes workspace");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_workspace_escape() {
        assert!(normalized_relative("../outside").is_err());
        assert!(normalized_relative("/tmp").is_err());
        assert!(normalized_relative("src/tests").is_ok());
    }

    #[test]
    fn validates_environment_keys() {
        assert!(validate_env_key("OPENFORGE_MODE").is_ok());
        assert!(validate_env_key("BAD=VALUE").is_err());
    }
}
