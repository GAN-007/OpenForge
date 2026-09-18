use anyhow::{Context,Result};
use globset::Glob;
use openforge_protocol::AutonomyLevel;
use serde::{Deserialize,Serialize};
use std::{fs,path::Path};

#[derive(Debug,Clone,Copy,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="snake_case")]
pub enum Decision{Allow,Ask,Deny}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct PatternRules{
    #[serde(default="default_ask")]pub default:Decision,
    #[serde(default)]pub allow:Vec<String>,
    #[serde(default)]pub ask:Vec<String>,
    #[serde(default)]pub deny:Vec<String>,
}
fn default_ask()->Decision{Decision::Ask}
impl Default for PatternRules{fn default()->Self{Self{default:Decision::Ask,allow:vec![],ask:vec![],deny:vec![]}}}
#[derive(Debug,Clone,Serialize,Deserialize,Default)]
pub struct FilesystemPolicy{#[serde(default)]pub read:PatternRules,#[serde(default)]pub write:PatternRules}
#[derive(Debug,Clone,Serialize,Deserialize,Default)]
pub struct AgentPolicy{
    #[serde(default)]pub autonomy:AutonomyLevel,
    #[serde(default)]pub filesystem:FilesystemPolicy,
    #[serde(default)]pub process:PatternRules,
    #[serde(default)]pub network:PatternRules,
    #[serde(default)]pub secrets:PatternRules,
    #[serde(default)]pub delegation:PatternRules,
}
#[derive(Debug,Clone,Copy)]
pub enum CapabilityRequest<'a>{ReadPath(&'a str),WritePath(&'a str),Process(&'a [String]),Network(&'a str),Secret(&'a str),Delegate(&'a str)}

impl AgentPolicy{
    pub fn from_yaml(path:impl AsRef<Path>)->Result<Self>{let raw=fs::read_to_string(path.as_ref()).with_context(||format!("read policy {}",path.as_ref().display()))?;Ok(serde_yaml::from_str(&raw).context("parse policy YAML")?)}
    pub fn evaluate(&self,request:CapabilityRequest<'_>)->Decision{
        let(rules,subject)=match request{
            CapabilityRequest::ReadPath(p)=>(&self.filesystem.read,p.to_string()),
            CapabilityRequest::WritePath(p)=>(&self.filesystem.write,p.to_string()),
            CapabilityRequest::Process(argv)=>(&self.process,shell_join(argv)),
            CapabilityRequest::Network(h)=>(&self.network,h.to_string()),
            CapabilityRequest::Secret(s)=>(&self.secrets,s.to_string()),
            CapabilityRequest::Delegate(a)=>(&self.delegation,a.to_string()),
        };
        self.apply_autonomy_ceiling(request,rules.evaluate(&subject))
    }
    fn apply_autonomy_ceiling(&self,request:CapabilityRequest<'_>,decision:Decision)->Decision{
        if decision==Decision::Deny{return Decision::Deny}
        match self.autonomy{
            AutonomyLevel::Observe=>match request{CapabilityRequest::ReadPath(_)=>decision,_=>Decision::Deny},
            AutonomyLevel::Suggest=>match request{CapabilityRequest::ReadPath(_)|CapabilityRequest::Network(_)=>decision,_=>Decision::Deny},
            AutonomyLevel::Edit=>match request{CapabilityRequest::Process(_)|CapabilityRequest::Secret(_)=>if decision==Decision::Allow{Decision::Ask}else{decision},_=>decision},
            AutonomyLevel::Execute|AutonomyLevel::Autonomous=>decision,
        }
    }
}
impl PatternRules{
    pub fn evaluate(&self,subject:&str)->Decision{
        if self.deny.iter().any(|p|matches(p,subject)){return Decision::Deny}
        if self.ask.iter().any(|p|matches(p,subject)){return Decision::Ask}
        if self.allow.iter().any(|p|matches(p,subject)){return Decision::Allow}
        self.default
    }
}
fn matches(pattern:&str,value:&str)->bool{Glob::new(pattern).map(|g|g.compile_matcher().is_match(value)).unwrap_or(false)}
fn shell_join(argv:&[String])->String{argv.iter().map(|a|if a.chars().all(|c|c.is_ascii_alphanumeric()||"-_./:=@".contains(c)){a.clone()}else{format!("'{}'",a.replace(''',"'\''"))}).collect::<Vec<_>>().join(" ")}
#[cfg(test)]
mod tests{use super::*;#[test]fn deny_dominates_allow(){let r=PatternRules{default:Decision::Ask,allow:vec!["**".into()],ask:vec![],deny:vec!["**/.env".into()]};assert_eq!(r.evaluate("app/.env"),Decision::Deny);assert_eq!(r.evaluate("src/lib.rs"),Decision::Allow);}}
