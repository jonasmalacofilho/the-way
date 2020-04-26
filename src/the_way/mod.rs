use std::collections::{HashMap, HashSet};

use anyhow::Error;
use chrono::{Date, DateTime, Datelike, Utc};
use clap::ArgMatches;
use clipboard::ClipboardContext;
use clipboard::ClipboardProvider;
use path_abs::{PathAbs, PathDir, PathFile, PathInfo, PathOps};

use crate::errors::LostTheWay;
use crate::language::Language;
use crate::the_way::filter::Filters;
use crate::the_way::snippet::Snippet;
use crate::{config, utils};

mod filter;
mod snippet;
mod stats;

/// Stores
/// - the directory to store files
/// - argument parsing information from `clap`
/// - the `sled` databases storing linkage information between languages, tags, and snippets
pub struct TheWay {
    dir: PathDir,
    matches: ArgMatches,
    db: sled::Db,
    languages: HashMap<String, Language>,
}

/// Reads config file to get location of the quoth directory
fn get_dir() -> Result<PathDir, Error> {
    match dirs::home_dir() {
        Some(home_dir) => {
            let config_file = PathAbs::new(PathDir::new(home_dir)?.join(config::CONFIG_PATH))?;
            if !config_file.exists() {
                make_config_file()?;
            }
            let dir_string = PathFile::new(config_file)?.read_string()?;
            Ok(PathDir::create_all(dir_string.trim())?)
        }
        None => Err(LostTheWay::Homeless.into()),
    }
}

/// Makes config file
/// default ~/the_way.txt) with a single line containing the location of the snippets directory (default ~/.the_way
fn make_config_file() -> Result<(), Error> {
    match dirs::home_dir() {
        Some(home_dir) => {
            let config_file = PathFile::create(PathDir::new(&home_dir)?.join(config::CONFIG_PATH))?;
            config_file.write_str(
                &PathDir::new(home_dir)?
                    .join(config::DIR_DEFAULT)
                    .to_str()
                    .unwrap(),
            )?;
            Ok(())
        }
        None => Err(LostTheWay::Homeless.into()),
    }
}

/// Changes the main snippets directory
fn change_dir(new_dir: &str) -> Result<(), Error> {
    match dirs::home_dir() {
        Some(home_dir) => {
            let config_file = PathFile::create(PathDir::new(home_dir)?.join(config::CONFIG_PATH))?;
            config_file.write_str(new_dir)?;
            Ok(())
        }
        None => Err(LostTheWay::Homeless.into()),
    }
}

/// If key exists, add value to existing values - join with a semicolon
fn merge_index(_key: &[u8], old_indices: Option<&[u8]>, new_index: &[u8]) -> Option<Vec<u8>> {
    let mut ret = old_indices
        .map(|old| old.to_vec())
        .unwrap_or_else(|| vec![]);
    if !ret.is_empty() {
        ret.extend_from_slice(&[utils::SEMICOLON]);
    }
    ret.extend_from_slice(new_index);
    Some(ret)
}

// All command-line related functions
impl TheWay {
    /// Initialize program with command line input.
    /// Reads `sled` trees and metadata file from the locations specified in config.
    /// (makes new ones the first time).
    pub(crate) fn start(
        matches: ArgMatches,
        languages: HashMap<String, Language>,
    ) -> Result<(), Error> {
        let dir = get_dir()?;
        let mut the_way = Self {
            db: Self::get_db(&dir)?,
            dir,
            matches,
            languages,
        };
        the_way.set_merge()?;
        the_way.run()?;
        // the_way.debug();
        Ok(())
    }

    /// Parses command-line arguments to decide which sub-command to run
    fn run(&mut self) -> Result<(), Error> {
        if self.matches.is_present("delete") {
            self.delete()
        } else if self.matches.is_present("show") {
            self.show()
        } else if self.matches.is_present("change") {
            self.change()
        } else if self.matches.is_present("copy") {
            self.copy()
        } else {
            match self.matches.subcommand() {
                ("config", Some(matches)) => self.config(matches),
                ("import", Some(matches)) => {
                    for snippet in self.import(matches)? {
                        self.add_snippet(&snippet)?;
                    }
                    Ok(())
                }
                ("export", Some(matches)) => self.export(matches),
                ("list", Some(matches)) => self.list(matches),
                ("search", Some(matches)) => self.search(matches),
                ("stats", Some(matches)) => self.stats(matches),
                _ => self.the_way(),
            }
        }
    }

    fn the_way(&mut self) -> Result<(), Error> {
        let snippet =
            Snippet::from_user(self.get_current_snippet_index()? + 1, &self.languages, None)?;
        println!("Added snippet #{}", self.add_snippet(&snippet)?);
        Ok(())
    }

    /// Delete a snippet (and all associated data) from the trees and metadata
    fn delete(&mut self) -> Result<(), Error> {
        let index = utils::get_argument_value("delete", &self.matches)?.ok_or(
            LostTheWay::OutOfCheeseError {
                message: "Argument delete not used".into(),
            },
        )?;
        let mut sure_delete;
        loop {
            sure_delete =
                utils::user_input(&format!("Delete snippet #{} Y/N?", index), Some("N"), true)?
                    .to_ascii_uppercase();
            if sure_delete == "Y" || sure_delete == "N" {
                break;
            }
        }
        if sure_delete == "Y" {
            let index = index.parse::<usize>()?;
            let snippet = self.remove_snippet(index)?;
            self.delete_from_trees(&snippet, index)?;
            println!("Snippet #{} deleted", index);
            Ok(())
        } else {
            Err(LostTheWay::DoingNothing {
                message: "I'm a coward.".into(),
            }
            .into())
        }
    }

    /// Modify a stored snippet's information
    fn change(&mut self) -> Result<(), Error> {
        let index = utils::get_argument_value("change", &self.matches)?
            .ok_or(LostTheWay::OutOfCheeseError {
                message: "Argument change not used".into(),
            })?
            .parse::<usize>()?;
        let old_snippet = self.get_snippet(index)?;
        self.delete_from_trees(&old_snippet, index)?;

        let new_snippet = Snippet::from_user(index, &self.languages, Some(old_snippet))?;
        let language_key = new_snippet.language.as_bytes();
        let index_key = index.to_string();
        let index_key = index_key.as_bytes();
        self.add_to_language(language_key, index_key)?;
        for tag in &new_snippet.tags {
            let tag_key = tag.as_bytes();
            self.tag_tree()?
                .merge(tag_key.to_vec(), index_key.to_vec())?;
        }
        self.snippets_tree()?
            .insert(index_key, new_snippet.to_bytes()?)?;

        println!("Snippet #{} changed", index); // TODO: change to log?
        Ok(())
    }

    // TODO: use syntect to display with syntax highlighting to terminal
    fn show(&self) -> Result<(), Error> {
        let index = utils::get_argument_value("show", &self.matches)?
            .ok_or(LostTheWay::OutOfCheeseError {
                message: "Argument show not used".into(),
            })?
            .parse::<usize>()?;
        let snippet = self.get_snippet(index)?;
        println!("{:?}", snippet);
        Ok(())
    }

    // Copy a snippet to clipboard
    fn copy(&self) -> Result<(), Error> {
        let index = utils::get_argument_value("copy", &self.matches)?
            .ok_or(LostTheWay::OutOfCheeseError {
                message: "Argument copy not used".into(),
            })?
            .parse::<usize>()?;
        let snippet = self.get_snippet(index)?;
        let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
        ctx.set_contents(snippet.code).unwrap();
        println!("Snippet #{} copied to clipboard", index);
        Ok(())
    }

    /// Clears all data or changes the snippets directory or generates shell completions
    fn config(&self, matches: &ArgMatches) -> Result<(), Error> {
        if matches.is_present("clear") {
            self.clear()
        } else if matches.is_present("dir") {
            self.relocate(matches)
        } else if matches.is_present("completions") {
            self.completions(matches)
        } else {
            Err(LostTheWay::OutOfCheeseError {
                message: "Unknown/No config argument".into(),
            }
            .into())
        }
    }

    fn import(&self, matches: &ArgMatches) -> Result<Vec<Snippet>, Error> {
        if matches.is_present("json") {
            let json_file = PathFile::new(utils::get_argument_value("json", matches)?.ok_or(
                LostTheWay::OutOfCheeseError {
                    message: "Argument json not used".into(),
                },
            )?)?;
            let snippets: Result<Vec<Snippet>, serde_json::Error> =
                Snippet::read_from_file(&json_file)?.collect();
            Ok(snippets?)
        } else {
            unimplemented!()
        }
    }

    // Saves (optionally filtered) snippets to an MD file
    fn export(&self, _matches: &ArgMatches) -> Result<(), Error> {
        // let filters = Filters::get_filters(matches)?;
        unimplemented!()
        // Ok(())
    }

    /// Lists snippets (optionally filtered)
    fn list(&self, matches: &ArgMatches) -> Result<(), Error> {
        let filters = Filters::get_filters(matches)?;
        let snippets = self.filter_snippets(&filters)?;
        for snippet in &snippets {
            snippet.pretty_print();
        }
        Ok(())
    }

    /// Searches the list of quotes (optionally filtered) for a pattern and displays quotes matching it
    fn search(&self, _matches: &ArgMatches) -> Result<(), Error> {
        // let pattern =
        //     utils::get_argument_value("pattern", matches)?.ok_or(LostTheWay::OutOfCheeseError {
        //         message: "Argument pattern not used".into(),
        //     })?;
        // let filters = Filters::get_filters(matches)?;
        // let snippets = self.filter_snippets(&filters)?;
        unimplemented!()
    }

    /// Generates shell completions
    fn completions(&self, _matches: &ArgMatches) -> Result<(), Error> {
        // let shell = utils::get_argument_value("completions", matches)?.ok_or(
        //     LostTheWay::OutOfCheeseError {
        //         message: "Argument shell not used".into(),
        //     },
        // )?;
        // let yaml = load_yaml!("../the_way.yml");
        // let mut app = App::from(yaml);
        unimplemented!()
        // app.gen_completions_to(
        //     "the_way",
        //     shell.parse::<Shell>().unwrap(),
        //     &mut io::stdout(),
        // );
        // Ok(())
    }

    /// Removes all `sled` trees
    fn clear(&self) -> Result<(), Error> {
        let mut sure_delete;
        loop {
            sure_delete =
                utils::user_input("Clear all data Y/N?", Some("N"), true)?.to_ascii_uppercase();
            if sure_delete == "Y" || sure_delete == "N" {
                break;
            }
        }
        if sure_delete == "Y" {
            PathDir::new(self.dir.join(config::DB_PATH))?.remove_all()?;
            Ok(())
        } else {
            Err(LostTheWay::DoingNothing {
                message: "I'm a coward.".into(),
            }
            .into())
        }
    }

    /// Changes the location of all `sled` trees and the metadata file
    fn relocate(&self, matches: &ArgMatches) -> Result<(), Error> {
        let new_dir =
            utils::get_argument_value("dir", matches)?.ok_or(LostTheWay::OutOfCheeseError {
                message: "Argument dir not used".into(),
            })?;
        let new_dir_path = PathDir::create_all(new_dir)?;
        if new_dir_path == self.dir {
            return Err(LostTheWay::DoingNothing {
                message: "Same as old dir.".into(),
            }
            .into());
        }
        let old_db = Self::get_db(&self.dir)?;
        let new_db = Self::get_db(&new_dir_path)?;
        new_db.import(old_db.export());
        self.clear()?;
        self.set_merge()?;
        change_dir(new_dir)?;
        let mut delete_old_dir;
        loop {
            delete_old_dir = utils::user_input("Delete old directory Y/N?", Some("N"), true)?
                .to_ascii_uppercase();
            if delete_old_dir == "Y" || delete_old_dir == "N" {
                break;
            }
        }
        if delete_old_dir == "Y" {
            self.dir.clone().remove_all()?;
            Ok(())
        } else {
            Err(LostTheWay::DoingNothing {
                message: "I'm a coward.".into(),
            }
            .into())
        }
    }
}

impl TheWay {
    pub fn debug(&self) {
        println!("{:?}", self.db);
    }

    /// Filters a list of snippets by given language/tag/date
    fn filter_snippets(&self, filters: &Filters<'_>) -> Result<Vec<Snippet>, Error> {
        let from_date = utils::date_start(filters.from_date);
        let to_date = utils::date_end(filters.to_date);
        let snippets: Option<Vec<_>> = match filters.languages.clone() {
            Some(languages) => Some(
                self.get_snippets(
                    &languages
                        .flat_map(|language| {
                            self.get_language_snippets(language).unwrap_or_default()
                        })
                        .collect::<Vec<_>>(),
                )?,
            ),
            None => None,
        };
        match (filters.tags.clone(), snippets) {
            (Some(tags), Some(snippets)) => Ok(snippets
                .into_iter()
                .filter(|snippet| {
                    snippet.in_date_range(from_date, to_date)
                        && tags.clone().any(|tag| snippet.has_tag(tag))
                })
                .collect()),
            (Some(tags), None) => {
                let indices = tags
                    .flat_map(|tag| self.get_tag_snippets(tag).unwrap_or_default())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                Snippet::filter_in_date_range(self.get_snippets(&indices)?, from_date, to_date)
            }
            (None, Some(snippets)) => Snippet::filter_in_date_range(snippets, from_date, to_date),
            (None, None) => self.list_snippets_in_date_range(from_date, to_date),
        }
    }

    fn get_db(dir: &PathDir) -> Result<sled::Db, Error> {
        Ok(sled::open(&PathDir::create_all(
            dir.join(config::DB_PATH),
        )?)?)
    }

    fn set_merge(&self) -> Result<(), Error> {
        self.language_tree()?.set_merge_operator(merge_index);
        self.tag_tree()?.set_merge_operator(merge_index);
        Ok(())
    }

    fn snippets_tree(&self) -> Result<sled::Tree, Error> {
        Ok(self.db.open_tree("snippets")?)
    }

    fn get_current_snippet_index(&self) -> Result<usize, Error> {
        match self.db.get("snippet_index")? {
            Some(index) => Ok(std::str::from_utf8(&index)?.parse::<usize>()?),
            None => Ok(0),
        }
    }

    fn language_tree(&self) -> Result<sled::Tree, Error> {
        Ok(self.db.open_tree("language_to_snippet")?)
    }

    fn tag_tree(&self) -> Result<sled::Tree, Error> {
        Ok(self.db.open_tree("tag_to_snippet")?)
    }

    /// Map a snippet index to a language
    fn add_to_language(&mut self, language_key: &[u8], index_key: &[u8]) -> Result<(), Error> {
        self.language_tree()?
            .merge(language_key.to_vec(), index_key.to_vec())?;
        Ok(())
    }

    fn get_snippet(&self, index: usize) -> Result<Snippet, Error> {
        let index_key = index.to_string();
        let index_key = index_key.as_bytes();
        Ok(Snippet::from_bytes(
            &self
                .snippets_tree()?
                .get(index_key)?
                .ok_or(LostTheWay::SnippetNotFound { index })?,
        )?)
    }

    fn get_snippets(&self, indices: &[usize]) -> Result<Vec<Snippet>, Error> {
        indices.iter().map(|i| self.get_snippet(*i)).collect()
    }

    /// List snippets in date range
    fn list_snippets_in_date_range(
        &self,
        from_date: DateTime<Utc>,
        to_date: DateTime<Utc>,
    ) -> Result<Vec<Snippet>, Error> {
        Ok(self
            .snippets_tree()?
            .iter()
            .map(|item| {
                item.map_err(|_| {
                    LostTheWay::OutOfCheeseError {
                        message: "sled PageCache Error".into(),
                    }
                    .into()
                })
                .and_then(|(_, snippet)| Snippet::from_bytes(&snippet))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|snippet| snippet.in_date_range(from_date, to_date))
            .collect())
    }

    fn increment_snippet_index(&mut self) -> Result<(), Error> {
        self.db.insert(
            "snippet_index",
            (self.get_current_snippet_index()? + 1)
                .to_string()
                .as_bytes(),
        )?;
        Ok(())
    }

    /// Add a snippet (with all attached data) to the database and change metadata accordingly
    fn add_snippet(&mut self, snippet: &Snippet) -> Result<usize, Error> {
        let language_key = snippet.language.as_bytes();
        let index_key = snippet.index.to_string();
        let index_key = index_key.as_bytes();
        self.snippets_tree()?
            .insert(index_key, snippet.to_bytes()?)?;
        self.add_to_language(language_key, index_key)?;
        for tag in &snippet.tags {
            let tag_key = tag.as_bytes();
            self.tag_tree()?
                .merge(tag_key.to_vec(), index_key.to_vec())?;
        }
        self.increment_snippet_index()?;
        Ok(snippet.index)
    }

    /// Delete a language (if no snippets are written in it)
    fn delete_language(&mut self, language_key: &[u8]) -> Result<(), Error> {
        self.language_tree()?.remove(language_key)?;
        Ok(())
    }

    /// Delete a snippet index from the language tree
    fn delete_from_language(&mut self, language_key: &[u8], index: usize) -> Result<(), Error> {
        let language = utils::u8_to_str(language_key)?;
        let new_indices: Vec<_> = utils::split_indices_usize(
            &self
                .language_tree()?
                .get(language_key)?
                .ok_or(LostTheWay::LanguageNotFound { language })?,
        )?
        .into_iter()
        .filter(|index_i| *index_i != index)
        .collect();
        if new_indices.is_empty() {
            self.delete_language(language_key)?;
        } else {
            self.language_tree()?.insert(
                language_key.to_vec(),
                utils::make_indices_string(&new_indices)?,
            )?;
        }
        Ok(())
    }

    /// Delete a snippet index from the tag tree
    fn delete_from_tag(
        &mut self,
        tag_key: &[u8],
        index: usize,
        batch: &mut sled::Batch,
    ) -> Result<(), Error> {
        let tag = utils::u8_to_str(tag_key)?;
        let new_indices: Vec<_> = utils::split_indices_usize(
            &self
                .tag_tree()?
                .get(tag_key)?
                .ok_or(LostTheWay::TagNotFound { tag })?,
        )?
        .into_iter()
        .filter(|index_i| *index_i != index)
        .collect();
        if new_indices.is_empty() {
            batch.remove(tag_key);
        } else {
            batch.insert(tag_key.to_vec(), utils::make_indices_string(&new_indices)?);
        }
        Ok(())
    }

    fn delete_from_trees(&mut self, snippet: &Snippet, index: usize) -> Result<(), Error> {
        let language_key = snippet.language.as_bytes();
        self.delete_from_language(language_key, index)?;
        let mut tag_batch = sled::Batch::default();
        for tag in &snippet.tags {
            self.delete_from_tag(tag.as_bytes(), index, &mut tag_batch)?;
        }
        self.tag_tree()?.apply_batch(tag_batch)?;
        Ok(())
    }

    fn remove_snippet(&mut self, index: usize) -> Result<Snippet, Error> {
        let index_key = index.to_string();
        let index_key = index_key.as_bytes();
        Ok(Snippet::from_bytes(
            &self
                .snippets_tree()?
                .remove(index_key)?
                .ok_or(LostTheWay::SnippetNotFound { index })?,
        )?)
    }

    /// Retrieve snippets written in a given language
    fn get_language_snippets(&self, language: &str) -> Result<Vec<usize>, Error> {
        utils::split_indices_usize(
            &self
                .language_tree()?
                .get(&language.to_ascii_lowercase().as_bytes())?
                .ok_or(LostTheWay::LanguageNotFound {
                    language: language.to_owned(),
                })?,
        )
    }

    /// Retrieve snippets associated with a given tag
    fn get_tag_snippets(&self, tag: &str) -> Result<Vec<usize>, Error> {
        utils::split_indices_usize(&self.tag_tree()?.get(tag.as_bytes())?.ok_or(
            LostTheWay::TagNotFound {
                tag: tag.to_owned(),
            },
        )?)
    }

    /// Get number of snippets per month
    fn get_snippet_counts_per_month(
        &self,
        from_date: DateTime<Utc>,
        to_date: DateTime<Utc>,
    ) -> Result<HashMap<Date<Utc>, u64>, Error> {
        let mut snippet_counts = HashMap::new();
        for snippet in self.list_snippets_in_date_range(from_date, to_date)? {
            *snippet_counts
                .entry(snippet.date.date().with_day(1).unwrap())
                .or_insert(0) += 1;
        }
        Ok(snippet_counts)
    }

    /// Get number of snippets per language for all languages stored
    fn get_language_counts(&self) -> Result<HashMap<String, u64>, Error> {
        let language_snippets: HashMap<String, u64> = self
            .language_tree()?
            .iter()
            .map(|item| {
                item.map_err(|_| LostTheWay::OutOfCheeseError {
                    message: "sled PageCache Error".into(),
                })
                .and_then(|(a, snippets)| {
                    match (utils::u8_to_str(&a), utils::split_indices_usize(&snippets)) {
                        (Ok(a), Ok(snippets)) => Ok((a, snippets.len() as u64)),
                        _ => Err(LostTheWay::OutOfCheeseError {
                            message: "Corrupt language_tree".into(),
                        }),
                    }
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(language_snippets)
    }
}
