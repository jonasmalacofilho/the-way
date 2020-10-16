use std::str;

use arboard::Clipboard;
use chrono::{Date, DateTime, Utc, MAX_DATE, MIN_DATE};
use chrono_english::{parse_date_string, Dialect};
use color_eyre::Help;
use dialoguer::{Editor, Input};

use crate::errors::LostTheWay;

/// To clear ANSI styling
pub const END_ANSI: &str = "\x1b[0m";

/// language color box
pub const BOX: &str = "\u{25a0}";

/// Name of the app, used for making project directories and reading the YAML file
pub const NAME: &str = "the-way";

/// ASCII code of semicolon
pub const SEMICOLON: u8 = 59;

/// Set clipboard contents to text
pub fn copy_to_clipboard(text: &str) -> color_eyre::Result<()> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text.into())?;
    Ok(())
}

/// Splits input by space
pub fn split_tags(input: &str) -> Vec<String> {
    input
        .split(' ')
        .map(|word| word.trim().to_string())
        .collect::<Vec<String>>()
}

/// Converts an array of bytes to a string
pub fn u8_to_str(input: &[u8]) -> color_eyre::Result<String> {
    Ok(str::from_utf8(input)?.to_owned())
}

/// Splits byte array by semicolon into usize
pub fn split_indices_usize(index_list: &[u8]) -> color_eyre::Result<Vec<usize>> {
    let index_list_string = str::from_utf8(index_list)?;
    Ok(index_list_string
        .split(str::from_utf8(&[SEMICOLON])?)
        .map(str::parse)
        .collect::<color_eyre::Result<Vec<_>, _>>()?)
}

/// List of usize into semicolon-joined byte array
pub fn make_indices_string(index_list: &[usize]) -> color_eyre::Result<Vec<u8>> {
    Ok(index_list
        .iter()
        .map(|index| index.to_string())
        .collect::<Vec<String>>()
        .join(str::from_utf8(&[SEMICOLON])?)
        .as_bytes()
        .to_vec())
}

/// Makes a date from a string, can be colloquial like "next Friday"
pub fn parse_date(date_string: &str) -> color_eyre::Result<Date<Utc>> {
    if date_string.to_ascii_lowercase() == "today" {
        Ok(Utc::now().date())
    } else {
        Ok(parse_date_string(date_string, Utc::now(), Dialect::Uk)?.date())
    }
}

/// Some(date) => date
/// None => minimum possible date
pub fn date_start(from_date: Option<Date<Utc>>) -> DateTime<Utc> {
    match from_date {
        Some(from_date) => from_date.and_hms(0, 0, 0),
        None => MIN_DATE.and_hms(0, 0, 0),
    }
}

/// Some(date) => date
/// None => maximum possible date
pub fn date_end(to_date: Option<Date<Utc>>) -> DateTime<Utc> {
    match to_date {
        Some(to_date) => to_date.and_hms(23, 59, 59),
        None => MAX_DATE.and_hms(23, 59, 59),
    }
}

/// Gets input from external editor, optionally displays default text in editor
pub fn external_editor_input(default: Option<&str>, extension: &str) -> color_eyre::Result<String> {
    Ok(Editor::new()
        .extension(extension)
        .edit(default.unwrap_or(""))
        .suggestion("Set your default editor using the $EDITOR or $VISUAL environment variables")?
        .ok_or(LostTheWay::EditorError)
        .suggestion("Make sure to save next time if you want to record a snippet!")?)
}

/// Takes user input from terminal, optionally has a default and optionally displays it.
pub fn user_input(
    message: &str,
    default: Option<&str>,
    show_default: bool,
    allow_empty: bool,
) -> color_eyre::Result<String> {
    let theme = dialoguer::theme::ColorfulTheme::default();
    match default {
        Some(default) => {
            let mut input = Input::with_theme(&theme);
            input
                .with_prompt(message)
                .allow_empty(allow_empty)
                .default(default.to_owned())
                .show_default(false);
            if show_default {
                input.with_initial_text(default);
            }
            // TODO: replace with `interact_text` on next dialoguer release
            Ok(input.interact()?.trim().to_owned())
        }
        None => Ok(Input::<String>::with_theme(&theme)
            .with_prompt(message)
            .allow_empty(allow_empty)
            // TODO: replace with `interact_text` on next dialoguer release
            .interact()?
            .trim()
            .to_owned()),
    }
}

pub fn get_spinner(message: &str) -> indicatif::ProgressBar {
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.enable_steady_tick(200);
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .tick_chars("/|\\- ")
            .template("{spinner:.dim.bold.blue} {wide_msg}"),
    );
    spinner.set_message(message);
    spinner
}
