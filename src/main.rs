use aho_corasick::{AhoCorasickBuilder, MatchKind};
use anyhow::{Context, Error};
use async_std::{fs::File, io::BufReader, prelude::*};
use colored::*;
use rayon::prelude::*;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    hash::Hash,
    path::PathBuf,
};
use structopt::StructOpt;
use walkdir::{DirEntry, WalkDir};

#[derive(StructOpt)]
struct Args {
    dir: PathBuf,
}

#[derive(Debug, Eq)]
struct Note {
    path: PathBuf,
    name: String,
    content: String,
}

impl Hash for Note {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state)
    }
}

impl PartialEq for Note {
    fn eq(&self, other: &Self) -> bool {
        self.path.eq(&other.path)
    }
}

#[async_std::main]
async fn main() -> Result<(), Error> {
    let args = Args::from_args();
    let paths = WalkDir::new(&args.dir).into_iter().filter_map(|e| e.ok());

    let mut fs = Vec::new();
    for path in paths {
        fs.push(process_file(path));
    }

    let notes: Vec<Note> = futures::future::try_join_all(fs)
        .await
        .context("failed while processing files")?
        .into_iter()
        .filter_map(|o| o)
        .collect();

    let links: HashMap<&Note, BTreeSet<&str>> = notes
        .par_iter()
        .map(|note| (note, wikilinks(&note.content)))
        .collect();

    let all_links: BTreeSet<&str> = links
        .iter()
        .flat_map(|(_, l)| l.iter().map(|s| *s))
        .filter(|l| !l.is_empty())
        .collect();

    let searcher = AhoCorasickBuilder::new()
        .match_kind(MatchKind::LeftmostLongest)
        .dfa(true)
        .build(all_links);

    notes.par_iter().for_each(|note| {
        let links = links.get(note).unwrap();

        let mut found = BTreeSet::new();
        for mat in searcher.find_iter(&note.content) {
            if let Some(c) = char_prior_to(mat.start(), &note.content) {
                if c.is_alphanumeric() {
                    continue;
                }
            }

            if let Some(c) = &note.content[mat.end()..].chars().next() {
                if c.is_alphanumeric() {
                    continue;
                }
            }

            found.insert(&note.content[mat.start()..mat.end()]);
        }

        let unlinked: Vec<&str> = found.difference(links).map(|s| *s).collect();
        if !unlinked.is_empty() {
            println!(
                "{}: {}",
                note.path
                    .strip_prefix(&args.dir)
                    .unwrap()
                    .as_os_str()
                    .to_string_lossy()
                    .blue(),
                unlinked.join(", ")
            );
        }
    });

    Ok(())
}

async fn process_file(e: DirEntry) -> Result<Option<Note>, Error> {
    let path = e.into_path();
    if !path.to_string_lossy().ends_with(".md") {
        return Ok(None);
    }

    let file = File::open(&path).await?;
    let metadata = file.metadata().await?;

    if metadata.is_dir() {
        return Ok(None);
    }

    let name = path
        .components()
        .last()
        .unwrap()
        .as_os_str()
        .to_string_lossy()
        .to_string();

    let mut reader = BufReader::new(file);
    let mut content = String::new();
    reader.read_to_string(&mut content).await?;

    Ok(Some(Note {
        name,
        path,
        content,
    }))
}

fn wikilinks(s: &str) -> BTreeSet<&str> {
    let mut n_brackets = 0;
    let mut in_wikilink = false;
    let mut in_pipe = false;
    let mut links = BTreeSet::new();
    let mut start = 0;
    let mut end = 0;

    for (i, c) in s.char_indices() {
        if c == '[' {
            if n_brackets == 0 {
                n_brackets += 1;
                continue;
            } else if n_brackets == 1 {
                n_brackets += 1;
                in_wikilink = true;
                start = i + c.len_utf8();
                continue;
            }
        }

        if c == ']' {
            if n_brackets == 2 {
                n_brackets -= 1;
                continue;
            } else if n_brackets == 1 {
                n_brackets -= 1;
                if in_wikilink {
                    in_wikilink = false;
                    in_pipe = false;
                    if end < start {
                        end = i - c.len_utf8();
                    }
                    links.insert(&s[start..end]);
                }
                continue;
            }
        }

        if !in_wikilink {
            continue;
        }

        if c == '|' && !in_pipe {
            in_pipe = true;
            end = i;
            continue;
        }

        if in_pipe {
            continue;
        }
    }

    links
}

fn char_prior_to(mut i: usize, s: &str) -> Option<char> {
    let bytes = s.as_bytes();
    if i > bytes.len() || i == 0 {
        return None;
    }

    let mut buf = Vec::with_capacity(4);
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        buf.push(b);
        if (b & 0b11000000) != 0b10000000 {
            break;
        }
    }

    if buf.is_empty() {
        return None;
    }

    buf.reverse();
    std::str::from_utf8(&buf).unwrap().chars().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("hello [[world]]" => vec!["world"])]
    #[test_case("[[hello]] [[world]]" => vec!["hello", "world"])]
    #[test_case("[[hello|what]] [[world|what]]" => vec!["hello", "world"])]
    #[test_case("[[hello\nworld]]" => vec!["hello\nworld"])]
    #[test_case("hello world 2" => Vec::<&str>::new())]
    #[test_case("" => Vec::<&str>::new() ; "empty 1")]
    #[test_case("[[" => Vec::<&str>::new() ; "empty 2")]
    #[test_case("]]" => Vec::<&str>::new() ; "empty 3")]
    #[test_case("[hello](there)" => Vec::<&str>::new() ; "normal link")]
    #[test_case("[[東]]" => vec!["東"]; "non-ascii")]
    #[test_case("[[wtf]][[東]]" => vec!["wtf", "東"]; "non-ascii 2")]
    #[test_case("[[東]][[wtf]]" => vec!["wtf", "東"]; "non-ascii 3")]
    #[test_case("[[東 hello]][[wtf]]" => vec!["wtf", "東 hello"]; "non-ascii 4")]
    #[test_case("[Senior Backend Engineer – Standard](https://standard.tv/pages/senior-engineer)" => Vec::<&str>::new(); "non-ascii 5")]
    fn test_wikilinks(s: &str) -> Vec<&str> {
        wikilinks(s).into_iter().collect()
    }

    #[test_case(1, "hello" => Some('h'))]
    #[test_case(0, "hello" => None)]
    #[test_case(10, "hello" => None)]
    #[test_case(3, "€10" => Some('€'))]
    #[test_case(2, "10€" => Some('0'))]
    #[test_case(10, "10€" => None)]
    #[test_case(0, "10€" => None)]
    fn test_char_prior_to(i: usize, s: &str) -> Option<char> {
        char_prior_to(i, s)
    }
}
