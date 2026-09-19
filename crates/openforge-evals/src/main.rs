use anyhow::{Context, Result, bail};
use clap::Parser;
use openforge_knowledge::KnowledgeGraph;
use openforge_policy::{AgentPolicy, CapabilityRequest, Decision};
use serde::Serialize;
use serde_json::Value;
use std::{fs, path::PathBuf};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[arg(long, default_value = "config/policies/development.yaml")]
    policy: PathBuf,
}

#[derive(Debug, Serialize)]
struct Gate {
    name: String,
    passed: bool,
    details: Value,
}

#[derive(Debug, Serialize)]
struct Report {
    version: String,
    passed: bool,
    gates: Vec<Gate>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = args.root.canonicalize().context("eval root not found")?;
    let mut gates = Vec::new();

    gates.push(validate_suites(&root)?);
    gates.push(validate_repository_knowledge(&root)?);
    gates.push(validate_security_policy(&root.join(args.policy))?);
    gates.push(validate_release_metadata(&root)?);

    let report = Report {
        version: env!("CARGO_PKG_VERSION").into(),
        passed: gates.iter().all(|gate| gate.passed),
        gates,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.passed {
        bail!("OpenForge release evaluation gates failed");
    }
    Ok(())
}

fn validate_suites(root: &std::path::Path) -> Result<Gate> {
    let evals = root.join("evals");
    let required = [
        "multi-agent/suite.json",
        "repository-understanding/suite.json",
        "routing/suite.json",
        "security/suite.json",
    ];
    let mut cases = 0usize;
    let mut suites = Vec::new();

    for relative in required {
        let path = evals.join(relative);
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read eval suite {}", path.display()))?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("parse eval suite {}", path.display()))?;
        let suite = value
            .get("suite")
            .and_then(Value::as_str)
            .context("eval suite missing suite name")?;
        let suite_cases = value
            .get("cases")
            .and_then(Value::as_array)
            .context("eval suite missing cases")?;
        if suite_cases.is_empty() {
            bail!("eval suite {suite} has no cases");
        }
        cases += suite_cases.len();
        suites.push(suite.to_string());
    }

    Ok(Gate {
        name: "eval-suites".into(),
        passed: cases >= 10,
        details: serde_json::json!({"suites": suites, "cases": cases}),
    })
}

fn validate_repository_knowledge(root: &std::path::Path) -> Result<Gate> {
    let graph = KnowledgeGraph::build(root)?;
    let symbols = graph
        .nodes
        .iter()
        .filter(|node| node.kind != openforge_knowledge::KnowledgeNodeKind::File)
        .count();
    let calls = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == openforge_knowledge::KnowledgeEdgeKind::Calls)
        .count();

    Ok(Gate {
        name: "repository-knowledge".into(),
        passed: graph.files_indexed > 20 && symbols > 50 && !graph.git_history.is_empty(),
        details: serde_json::json!({
            "files": graph.files_indexed,
            "symbols": symbols,
            "edges": graph.edges.len(),
            "calls": calls,
            "git_history_files": graph.git_history.len()
        }),
    })
}

fn validate_security_policy(path: &std::path::Path) -> Result<Gate> {
    let policy = AgentPolicy::from_yaml(path)?;
    let secret_denied = policy.evaluate(CapabilityRequest::ReadPath("home/user/.ssh/id_ed25519"))
        == Decision::Deny
        || policy.evaluate(CapabilityRequest::ReadPath("~/.ssh/id_ed25519")) == Decision::Deny;
    let deploy_not_allowed =
        policy.evaluate(CapabilityRequest::Deployment("prod/api")) != Decision::Allow;
    let secret_capability_denied =
        policy.evaluate(CapabilityRequest::Secret("OPENAI_API_KEY")) == Decision::Deny;

    Ok(Gate {
        name: "security-policy".into(),
        passed: secret_denied && deploy_not_allowed && secret_capability_denied,
        details: serde_json::json!({
            "secret_path_denied": secret_denied,
            "production_deployment_not_auto_allowed": deploy_not_allowed,
            "secret_capability_denied": secret_capability_denied
        }),
    })
}

fn validate_release_metadata(root: &std::path::Path) -> Result<Gate> {
    let readme = fs::read_to_string(root.join("README.md"))?;
    let cargo = fs::read_to_string(root.join("Cargo.toml"))?;
    let protocol = fs::read_to_string(root.join("crates/openforge-protocol/src/lib.rs"))?;
    let passed = readme.contains("Apache-2.0")
        && cargo.contains("version = \"0.3.0\"")
        && protocol.contains("openforge.protocol.v");
    Ok(Gate {
        name: "release-metadata".into(),
        passed,
        details: serde_json::json!({"version": env!("CARGO_PKG_VERSION")}),
    })
}
