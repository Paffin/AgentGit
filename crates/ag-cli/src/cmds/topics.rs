//! `ag help [TOPIC | COMMAND…]`: concept pages next to the per-command help.
//!
//! The pages are the files in `crates/ag-cli/help/`. The same files are rendered into
//! `docs/reference/help-topics.md` by `scripts/gen_cli_docs.py`, so the terminal and the documentation site
//! cannot disagree.

use anyhow::{Result, bail};
use clap::{Args, CommandFactory};

use crate::exit;
use crate::ui;

/// Every topic: its name and its text. The first line of the text is its title.
pub const TOPICS: &[(&str, &str)] = &[
    ("workflow", include_str!("../../help/workflow.md")),
    ("policy", include_str!("../../help/policy.md")),
    ("leases", include_str!("../../help/leases.md")),
    ("identities", include_str!("../../help/identities.md")),
    ("revisions", include_str!("../../help/revisions.md")),
    ("exit-codes", include_str!("../../help/exit-codes.md")),
    ("json", include_str!("../../help/json.md")),
    ("config", include_str!("../../help/config.md")),
];

/// The text of a topic.
pub fn topic(name: &str) -> Option<&'static str> {
    TOPICS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

#[derive(Args, Debug)]
pub struct HelpArgs {
    /// A topic (`ag help policy`) or a command (`ag help stream create`). Without it: the overview and the topics.
    #[arg(value_name = "TOPIC | COMMAND")]
    pub what: Vec<String>,
}

pub fn help(a: &HelpArgs) -> Result<u8> {
    let color = ui::settings().color_out;
    let mut cmd = crate::Cli::command();
    cmd.build();
    match a.what.as_slice() {
        [] => {
            let h = cmd.render_long_help();
            ui::print_line(h.to_string().trim_end());
            ui::print_line("");
            ui::print_line(&topic_list());
        }
        [one] if topic(one).is_some() => {
            let text = topic(one).unwrap_or_default();
            let mut lines = text.trim_end().lines();
            let title = lines.next().unwrap_or_default();
            ui::print_line(&ui::finish(&ui::tone(ui::Tone::Strong, title), color));
            for l in lines {
                ui::print_line(l);
            }
        }
        path => {
            let mut cur = &mut cmd;
            for name in path {
                let Some(next) = cur.find_subcommand_mut(name) else {
                    bail!(
                        "no help topic or command '{}'; topics: {}; commands: see `ag --help`",
                        path.join(" "),
                        TOPICS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
                    );
                };
                cur = next;
            }
            let h = cur.render_long_help();
            let text = if color { h.ansi().to_string() } else { h.to_string() };
            ui::print_line(text.trim_end());
        }
    }
    Ok(exit::OK)
}

/// One line per topic: `  name   title`.
pub fn topic_list() -> String {
    let mut s = String::from("Help topics (ag help TOPIC):\n");
    for (name, text) in TOPICS {
        s.push_str(&format!("  {name:<11}{}\n", text.lines().next().unwrap_or_default()));
    }
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_title_and_is_listed_in_the_root_help() {
        let root = include_str!("../main.rs");
        for (name, text) in TOPICS {
            assert!(text.lines().next().is_some_and(|t| !t.trim().is_empty()), "{name} has no title");
            assert!(root.contains(name), "the root help does not mention the topic '{name}'");
        }
    }

    #[test]
    fn topics_only_mention_commands_that_exist() {
        let cmd = crate::Cli::command();
        let names: Vec<String> = cmd.get_subcommands().map(|c| c.get_name().to_string()).collect();
        for (topic, text) in TOPICS {
            for line in text.lines() {
                let mut rest = line;
                while let Some(i) = rest.find("ag ") {
                    let tail = &rest[i + 3..];
                    let word: String = tail.chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
                    let boundary_ok = i == 0 || !rest[..i].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '-');
                    let named = word.chars().next().is_some_and(|c| c.is_ascii_lowercase());
                    if boundary_ok
                        && named
                        && word.len() > 1
                        && !["is", "the", "and", "to", "or", "for", "a", "in", "can"].contains(&word.as_str())
                    {
                        assert!(
                            names.contains(&word) || word == "server",
                            "topic '{topic}' mentions `ag {word}`, which is not a command"
                        );
                    }
                    rest = &rest[i + 3..];
                }
            }
        }
    }
}
