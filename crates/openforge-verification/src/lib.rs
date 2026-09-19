use anyhow::{Context, Result, bail};
use openforge_sandbox::{ExecRequest, ExecResult, SandboxBackend, SandboxLease};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStage {
    Compile,
    Lint,
    Test,
    Runtime,
    Review,
    SecurityReview,
    Final,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCommand {
    pub stage: VerificationStage,
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationPlan {
    pub commands: Vec<VerificationCommand>,
    pub model_review_required: bool,
    pub security_review_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStepResult {
    pub command: VerificationCommand,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerificationReport {
    pub steps: Vec<VerificationStepResult>,
}

impl VerificationReport {
    pub fn passed(&self) -> bool {
        self.steps
            .iter()
            .all(|step| step.passed || !step.command.required)
    }

    pub fn require_passed(&self) -> Result<()> {
        if self.passed() {
            return Ok(());
        }
        let failures = self
            .steps
            .iter()
            .filter(|step| step.command.required && !step.passed)
            .map(|step| format!("{:?}: {:?}", step.command.stage, step.command.argv))
            .collect::<Vec<_>>();
        bail!("verification failed: {}", failures.join(", "))
    }
}

impl VerificationPlan {
    pub fn detect(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let mut commands = Vec::new();

        if root.join("Cargo.toml").exists() {
            commands.extend([
                command(
                    VerificationStage::Compile,
                    &["cargo", "check", "--workspace", "--all-targets"],
                    900,
                ),
                command(
                    VerificationStage::Lint,
                    &["cargo", "fmt", "--all", "--check"],
                    300,
                ),
                command(
                    VerificationStage::Lint,
                    &[
                        "cargo",
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                    1200,
                ),
                command(
                    VerificationStage::Test,
                    &["cargo", "test", "--workspace"],
                    1800,
                ),
            ]);
        }

        if root.join("package.json").exists() {
            let raw = fs::read_to_string(root.join("package.json"))
                .context("read package.json for verification detection")?;
            let value: serde_json::Value = serde_json::from_str(&raw)
                .context("parse package.json for verification detection")?;
            let scripts = value
                .get("scripts")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();

            for (name, stage, timeout) in [
                ("typecheck", VerificationStage::Compile, 900),
                ("lint", VerificationStage::Lint, 900),
                ("test", VerificationStage::Test, 1800),
                ("build", VerificationStage::Compile, 1800),
            ] {
                if scripts.contains_key(name) {
                    commands.push(VerificationCommand {
                        stage,
                        argv: vec!["pnpm".into(), name.into()],
                        cwd: ".".into(),
                        timeout_seconds: timeout,
                        required: true,
                    });
                }
            }
        }

        if root.join("pyproject.toml").exists() || root.join("python/pyproject.toml").exists() {
            let cwd = if root.join("python/pyproject.toml").exists() {
                "python"
            } else {
                "."
            };
            commands.extend([
                VerificationCommand {
                    stage: VerificationStage::Lint,
                    argv: vec!["ruff".into(), "check".into(), ".".into()],
                    cwd: cwd.into(),
                    timeout_seconds: 600,
                    required: true,
                },
                VerificationCommand {
                    stage: VerificationStage::Test,
                    argv: vec!["pytest".into(), "-q".into()],
                    cwd: cwd.into(),
                    timeout_seconds: 1200,
                    required: true,
                },
            ]);
        }

        commands.push(VerificationCommand {
            stage: VerificationStage::Final,
            argv: vec!["git".into(), "diff".into(), "--check".into()],
            cwd: ".".into(),
            timeout_seconds: 120,
            required: true,
        });

        Ok(Self {
            commands,
            model_review_required: true,
            security_review_required: true,
        })
    }

    pub fn grouped(&self) -> BTreeMap<VerificationStage, Vec<&VerificationCommand>> {
        let mut grouped = BTreeMap::new();
        for command in &self.commands {
            grouped
                .entry(command.stage)
                .or_insert_with(Vec::new)
                .push(command);
        }
        grouped
    }
}

pub struct Verifier;

impl Verifier {
    pub async fn run(
        backend: &dyn SandboxBackend,
        lease: &SandboxLease,
        plan: &VerificationPlan,
    ) -> Result<VerificationReport> {
        let mut report = VerificationReport::default();

        for command in &plan.commands {
            if command.argv.is_empty() {
                bail!("verification command argv cannot be empty");
            }
            let result = backend
                .exec(
                    lease,
                    ExecRequest {
                        argv: command.argv.clone(),
                        cwd: command.cwd.clone(),
                        environment: BTreeMap::new(),
                        timeout_seconds: command.timeout_seconds.max(1),
                    },
                )
                .await
                .with_context(|| format!("execute verification command {:?}", command.argv))?;
            report.steps.push(step_result(command.clone(), result));
            if command.required && !report.steps.last().expect("step exists").passed {
                break;
            }
        }

        Ok(report)
    }
}

fn command(stage: VerificationStage, argv: &[&str], timeout_seconds: u64) -> VerificationCommand {
    VerificationCommand {
        stage,
        argv: argv.iter().map(|value| (*value).to_string()).collect(),
        cwd: ".".into(),
        timeout_seconds,
        required: true,
    }
}

fn step_result(command: VerificationCommand, result: ExecResult) -> VerificationStepResult {
    VerificationStepResult {
        command,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        timed_out: result.timed_out,
        passed: result.exit_code == 0 && !result.timed_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_plan_keeps_stage_orderability() {
        let plan = VerificationPlan {
            commands: vec![command(VerificationStage::Test, &["true"], 1)],
            model_review_required: true,
            security_review_required: true,
        };
        assert_eq!(plan.grouped().len(), 1);
    }
}
