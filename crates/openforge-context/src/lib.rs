use anyhow::{Context,Result};
use ignore::WalkBuilder;
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use std::{collections::{BTreeMap,BTreeSet},fs,path::{Path,PathBuf}};

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct FileRecord{
    pub path:String,
    pub bytes:u64,
    pub sha256:String,
    pub language:String,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct RepositoryIndex{
    pub root:String,
    pub files:Vec<FileRecord>,
    pub language_counts:BTreeMap<String,usize>,
}
impl RepositoryIndex{
    pub fn build(root:impl AsRef<Path>)->Result<Self>{
        let root=root.as_ref().canonicalize().context("repository root unavailable")?;
        let mut files=Vec::new();
        let mut language_counts=BTreeMap::new();
        for entry in WalkBuilder::new(&root).hidden(false).git_ignore(true).git_exclude(true).build(){
            let entry=entry?;
            if !entry.file_type().map(|t|t.is_file()).unwrap_or(false){continue}
            let p=entry.path();
            if p.components().any(|c| c.as_os_str()==".git"){continue}
            let data=fs::read(p)?;
            if data.len()>5*1024*1024 || data.iter().take(8192).any(|b|*b==0){continue}
            let rel=p.strip_prefix(&root)?.to_string_lossy().replace('\\',"/");
            let language=language_for(p);
            *language_counts.entry(language.clone()).or_insert(0)+=1;
            files.push(FileRecord{path:rel,bytes:data.len() as u64,sha256:hex::encode(Sha256::digest(&data)),language});
        }
        files.sort_by(|a,b|a.path.cmp(&b.path));
        Ok(Self{root:root.display().to_string(),files,language_counts})
    }

    pub fn relevant_files(&self,query:&str,limit:usize)->Vec<String>{
        let terms:BTreeSet<String>=query.split(|c:char|!c.is_alphanumeric()&&c!='_').filter(|s|s.len()>2).map(|s|s.to_lowercase()).collect();
        let mut scored:Vec<(i32,String)>=self.files.iter().map(|f|{
            let lower=f.path.to_lowercase();
            let score=terms.iter().map(|t|if lower.contains(t){4}else{0}).sum::<i32>()
                + if lower.contains("test"){1}else{0};
            (score,f.path.clone())
        }).filter(|(s,_)|*s>0).collect();
        scored.sort_by(|a,b|b.0.cmp(&a.0).then_with(||a.1.cmp(&b.1)));
        scored.into_iter().take(limit).map(|(_,p)|p).collect()
    }
}
fn language_for(path:&Path)->String{
    match path.extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase().as_str(){
        "rs"=>"rust","ts"|"tsx"=>"typescript","js"|"jsx"=>"javascript","py"=>"python","go"=>"go","java"=>"java","kt"|"kts"=>"kotlin",
        "rb"=>"ruby","php"=>"php","cs"=>"csharp","c"|"h"=>"c","cpp"|"cc"|"hpp"=>"cpp","sql"=>"sql","html"=>"html","css"=>"css",
        "json"=>"json","yaml"|"yml"=>"yaml","toml"=>"toml","md"=>"markdown","sh"=>"shell",_=>"text"
    }.into()
}
