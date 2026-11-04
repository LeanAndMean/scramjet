//! The two lines under the prompt, as Scramjet's footer: the project's directory with its Git
//! branch and the session's name, then the session's tokens with the model's output rate, the
//! cost, how full the context is, and the model with its provider and effort.

use super::settings::scramjet_level;
use super::style::{FOOTER, FOOTER_FULL, FOOTER_NEARLY_FULL, RESET};
use super::terminal::{dollars, rate, tokens};
use super::text::{clip, one_line, visible};
use scramjet_rs::agent::{Agent, Tokens};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

/// How often the branch is read again, so that a checkout shows about as soon as Scramjet's
/// watcher shows it.
const BRANCH_CHECK: Duration = Duration::from_secs(1);

/// What the footer shows of the session, as the session sends it to the screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Shown {
    pub(super) name: Option<String>,
    pub(super) tokens: Tokens,
    /// How full the context is (`Agent::context_tokens`), out of `window` when known.
    pub(super) context: Option<u64>,
    pub(super) window: Option<u64>,
    /// The model's median output rate, in tokens a second (`Agent::output_rate`).
    pub(super) rate: Option<f64>,
    pub(super) model: String,
    /// The model's provider, when the endpoint has several (`Agent::provider`).
    pub(super) provider: Option<String>,
    /// The effort level, for a model that reasons.
    pub(super) effort: Option<String>,
}

impl Shown {
    pub(super) fn of(agent: &Agent) -> Self {
        Self {
            name: agent.session_name().map(str::to_owned),
            tokens: agent.session_tokens(),
            context: agent.context_tokens(),
            window: agent.context_window(),
            rate: agent.output_rate(),
            model: agent.model().to_owned(),
            provider: agent.provider().map(str::to_owned),
            effort: (agent.effort())
                .filter(|_| agent.reasons())
                .map(str::to_owned),
        }
    }
}

pub(super) struct Footer {
    /// The project's directory, with `~` for the home directory.
    directory: String,
    head: Option<Head>,
    branch: Option<String>,
    checked: Instant,
    pub(super) shown: Shown,
}

impl Footer {
    pub(super) fn new(root: &Path) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let directory = match home.and_then(|home| root.strip_prefix(home).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Some(rest) => format!("~/{}", rest.display()),
            None => root.display().to_string(),
        };
        let head = head(root);
        Self {
            directory,
            branch: head.as_ref().and_then(Head::branch),
            head,
            checked: Instant::now(),
            shown: Shown::default(),
        }
    }

    /// Reads the branch again once `BRANCH_CHECK` has passed since it was read.
    pub(super) fn check_branch(&mut self) {
        if let Some(head) = &self.head
            && self.checked.elapsed() >= BRANCH_CHECK
        {
            self.branch = head.branch();
            self.checked = Instant::now();
        }
    }

    /// The footer's lines on a screen `width` wide, each in at most `width - 1` columns, as the
    /// screen's other rows.
    pub(super) fn lines(&self, width: usize) -> [String; 2] {
        let room = width.saturating_sub(1);
        let mut place = self.directory.clone();
        if let Some(branch) = &self.branch {
            place += &format!(" ({branch})");
        }
        if let Some(name) = &self.shown.name {
            place += &format!(" • {name}");
        }
        let place = clip(&visible(&one_line(&place)), room);
        [format!("{FOOTER}{place}{RESET}"), self.stats(room)]
    }

    /// As Scramjet's: the tokens that are not zero, the output's with the model's rate, what was
    /// spent, how full the context is (in the warning color past 70%, the error color past 90%),
    /// with `auto` as compaction runs by itself, and at the right when there is room the model
    /// with its effort, after its provider when that fits too.
    fn stats(&self, room: usize) -> String {
        let shown = &self.shown;
        let output_rate = (shown.rate).map_or_else(String::new, |r| format!(" ({})", rate(r)));
        let mut counts = Vec::new();
        for (mark, count, after) in [
            ("↑", shown.tokens.input, ""),
            ("↓", shown.tokens.output, output_rate.as_str()),
            ("R", shown.tokens.cache_read, ""),
            ("W", shown.tokens.cache_write, ""),
        ] {
            if count > 0 {
                counts.push(format!("{mark}{}{after}", tokens(count)));
            }
        }
        counts.extend(dollars(shown.tokens.cost, shown.tokens.unpriced, 3));
        let (full, percent) = match (shown.context, shown.window) {
            (count, Some(window @ 1..)) => {
                let percent = count.map(|count| count as f64 * 100.0 / window as f64);
                let count = count.map_or_else(|| "?".to_owned(), tokens);
                let shown = percent.map_or_else(|| "?".to_owned(), |p| format!("{p:.1}"));
                let window = tokens(window);
                (format!("{count}/{window} ({shown}%, auto)"), percent)
            }
            (Some(count), _) => (tokens(count), None),
            (None, _) => ("?".to_owned(), None),
        };
        let before = match counts.is_empty() {
            true => String::new(),
            false => counts.join(" ") + " ",
        };
        let left = before.clone() + &full;
        if left.width() > room {
            return format!("{FOOTER}{}{RESET}", clip(&left, room));
        }
        let model = match shown.effort.as_deref().map(scramjet_level) {
            Some("off") => format!("{} • thinking off", shown.model),
            Some(level) => format!("{} • {level}", shown.model),
            None => shown.model.clone(),
        };
        let model = one_line(&model);
        let named =
            (shown.provider.as_deref()).map(|provider| one_line(&format!("({provider}) {model}")));
        let right = match room.checked_sub(left.width() + 2) {
            Some(fits) if named.as_ref().is_some_and(|named| fits >= named.width()) => {
                named.unwrap_or_default()
            }
            Some(fits) if fits >= model.width() => model,
            Some(fits) if fits > 0 => cut(&model, fits),
            _ => String::new(),
        };
        let padding = match right.is_empty() {
            true => String::new(),
            false => " ".repeat(room - left.width() - right.width()),
        };
        let full_style = match percent {
            Some(percent) if percent > 90.0 => FOOTER_FULL,
            Some(percent) if percent > 70.0 => FOOTER_NEARLY_FULL,
            _ => FOOTER,
        };
        format!("{FOOTER}{before}{RESET}{full_style}{full}{RESET}{FOOTER}{padding}{right}{RESET}")
    }
}

/// `text`'s first characters that fit in `width` columns, as Scramjet cuts the model's name.
fn cut(text: &str, width: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
            used <= width
        })
        .collect()
}

/// Where the repository the project is in names its checked-out branch, as Scramjet finds it:
/// the `HEAD` of the nearest `.git` directory, or of the directory a `.git` file names (a
/// worktree's).
struct Head {
    file: PathBuf,
    repository: PathBuf,
}

fn head(root: &Path) -> Option<Head> {
    for directory in root.ancestors() {
        let git = directory.join(".git");
        let Ok(metadata) = std::fs::metadata(&git) else {
            continue;
        };
        let git_directory = if metadata.is_file() {
            let text = std::fs::read_to_string(&git).ok()?;
            directory.join(text.trim().strip_prefix("gitdir: ")?.trim())
        } else if metadata.is_dir() {
            git
        } else {
            continue;
        };
        let file = git_directory.join("HEAD");
        return file.exists().then(|| Head {
            file,
            repository: directory.to_owned(),
        });
    }
    None
}

impl Head {
    /// The branch checked out, `detached` when none is; `None` when `HEAD` cannot be read.
    fn branch(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        Some(match text.trim().strip_prefix("ref: refs/heads/") {
            // A reftable repository names its branch only through Git.
            Some(".invalid") => self.symbolic_ref().unwrap_or_else(|| "detached".to_owned()),
            Some(branch) => branch.to_owned(),
            None => "detached".to_owned(),
        })
    }

    fn symbolic_ref(&self) -> Option<String> {
        let output = Command::new("git")
            .args([
                "--no-optional-locks",
                "symbolic-ref",
                "--quiet",
                "--short",
                "HEAD",
            ])
            .current_dir(&self.repository)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !branch.is_empty()).then_some(branch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        super::super::text::plain(text)
    }

    fn footer(shown: Shown) -> Footer {
        Footer {
            directory: "~/repos/project".into(),
            head: None,
            branch: Some("main".into()),
            checked: Instant::now(),
            shown,
        }
    }

    fn opus() -> Shown {
        Shown {
            name: Some("Refactor".into()),
            tokens: Tokens {
                input: 12_345,
                output: 3_400,
                cache_read: 104_000,
                ..Tokens::default()
            },
            context: Some(53_000),
            window: Some(1_000_000),
            rate: None,
            model: "claude-opus-5-5".into(),
            provider: None,
            effort: Some("high".into()),
        }
    }

    #[test]
    fn the_lines_say_where_what_was_spent_and_the_model_as_scramjets() {
        let [place, stats] = footer(opus()).lines(67).map(|line| plain(&line));
        assert_eq!(place, "~/repos/project (main) • Refactor");
        assert_eq!(
            stats,
            "↑12k ↓3.4k R104k 53k/1.0M (5.3%, auto)      claude-opus-5-5 • high"
        );
        assert_eq!(stats.width(), 66);
        // Tokens never counted are left out; `none` is Scramjet's `off`.
        let fresh = Shown {
            name: None,
            tokens: Tokens::default(),
            context: Some(0),
            effort: Some("none".into()),
            ..opus()
        };
        let [place, stats] = footer(fresh).lines(61).map(|line| plain(&line));
        assert_eq!(place, "~/repos/project (main)");
        assert!(
            stats.starts_with("0/1.0M (0.0%, auto) ")
                && stats.ends_with(" claude-opus-5-5 • thinking off"),
            "{stats}"
        );
        // A model that does not reason shows no effort.
        let plain_model = Shown {
            effort: None,
            ..opus()
        };
        assert!(plain(&footer(plain_model).lines(61)[1]).ends_with(" claude-opus-5-5"));
    }

    #[test]
    fn cache_writes_and_the_cost_follow_the_tokens_with_what_is_not_known() {
        let spent = |cost, unpriced| {
            let tokens = Tokens {
                cache_write: 2_000,
                cost,
                unpriced,
                ..opus().tokens
            };
            let shown = Shown { tokens, ..opus() };
            plain(&footer(shown).lines(80)[1])
        };
        assert_eq!(
            spent(0.4567, false),
            "↑12k ↓3.4k R104k W2.0k $0.457 53k/1.0M (5.3%, auto)      claude-opus-5-5 • high"
        );
        // Responses without a price, or saved before costs were, leave the total open.
        assert!(spent(0.4567, true).starts_with("↑12k ↓3.4k R104k W2.0k $0.457+? 53k/1.0M "));
        assert!(spent(0.0, true).starts_with("↑12k ↓3.4k R104k W2.0k $? 53k/1.0M "));
        assert!(spent(0.0, false).starts_with("↑12k ↓3.4k R104k W2.0k 53k/1.0M "));
    }

    #[test]
    fn an_unknown_context_shows_question_marks() {
        let unknown = Shown {
            context: None,
            ..opus()
        };
        let stats = plain(&footer(unknown.clone()).lines(80)[1]);
        assert!(
            stats.starts_with("↑12k ↓3.4k R104k ?/1.0M (?%, auto) "),
            "{stats}"
        );
        let no_card = Shown {
            window: None,
            ..unknown
        };
        assert!(plain(&footer(no_card).lines(80)[1]).starts_with("↑12k ↓3.4k R104k ? "));
        let count_only = Shown {
            window: None,
            ..opus()
        };
        assert!(plain(&footer(count_only).lines(80)[1]).starts_with("↑12k ↓3.4k R104k 53k "));
    }

    #[test]
    fn the_output_shows_the_models_rate_and_the_model_its_provider_when_both_fit() {
        let shown = Shown {
            rate: Some(52.4),
            provider: Some("anthropic".into()),
            ..opus()
        };
        let stats = |shown: &Shown, width| plain(&footer(shown.clone()).lines(width)[1]);
        assert_eq!(
            stats(&shown, 85),
            "↑12k ↓3.4k (52tok/s) R104k 53k/1.0M (5.3%, auto)  (anthropic) claude-opus-5-5 • high"
        );
        // Without room for the provider, the model shows without it before it is cut.
        assert_eq!(
            stats(&shown, 84),
            "↑12k ↓3.4k (52tok/s) R104k 53k/1.0M (5.3%, auto)             claude-opus-5-5 • high"
        );
        // Nothing written yet shows no rate.
        let unwritten = Shown {
            tokens: Tokens::default(),
            ..shown
        };
        assert!(stats(&unwritten, 85).starts_with("53k/1.0M (5.3%, auto) "));
    }

    #[test]
    fn a_narrow_screen_cuts_the_model_then_the_counts() {
        let stats = |width| plain(&footer(opus()).lines(width)[1]);
        // The model's name is cut without a mark, two columns after the counts.
        assert_eq!(
            stats(51),
            "↑12k ↓3.4k R104k 53k/1.0M (5.3%, auto)  claude-opu"
        );
        assert_eq!(stats(41), "↑12k ↓3.4k R104k 53k/1.0M (5.3%, auto)");
        assert_eq!(stats(20), "↑12k ↓3.4k R104k 5…");
        let [place, _] = footer(opus()).lines(16).map(|line| plain(&line));
        assert_eq!(place, "~/repos/projec…");
    }

    #[test]
    fn a_nearly_full_context_shows_in_the_warning_then_the_error_color() {
        let at = |count| {
            let shown = Shown {
                context: Some(count),
                ..opus()
            };
            footer(shown).lines(80)[1].clone()
        };
        let styled = |style: &str, text: &str| format!("{style}{text}{RESET}");
        assert!(at(700_000).contains(&styled(FOOTER, "700k/1.0M (70.0%, auto)")));
        let nearly = styled(FOOTER_NEARLY_FULL, "700k/1.0M (70.0%, auto)");
        assert!(at(700_001).contains(&nearly));
        assert!(at(950_000).contains(&styled(FOOTER_FULL, "950k/1.0M (95.0%, auto)")));
    }

    #[test]
    fn names_are_shown_on_one_line_with_control_characters_escaped() {
        let named = Shown {
            name: Some("two\nlines \u{202e}reversed".into()),
            ..opus()
        };
        let place = plain(&footer(named).lines(80)[0]);
        assert_eq!(
            place,
            "~/repos/project (main) • two lines \\u{202e}reversed"
        );
    }

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_branch_is_read_from_the_repository_the_project_is_in() {
        let guard = Scratch(
            std::env::temp_dir().join(format!("scramjet-rs-footer-{}", std::process::id())),
        );
        let scratch = &guard.0;
        let _ = std::fs::remove_dir_all(scratch);
        let project = scratch.join("repo/src/deep");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(scratch.join("repo/.git")).unwrap();
        let head_file = scratch.join("repo/.git/HEAD");
        std::fs::write(&head_file, "ref: refs/heads/feature/footer\n").unwrap();
        let branch = |root: &Path| head(root).and_then(|head| head.branch());
        assert_eq!(branch(&project).as_deref(), Some("feature/footer"));
        std::fs::write(&head_file, "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        assert_eq!(branch(&project).as_deref(), Some("detached"));
        // A worktree's `.git` file names its own directory, relative or not.
        let worktree = scratch.join("worktree");
        std::fs::create_dir_all(scratch.join("repo/.git/worktrees/w")).unwrap();
        std::fs::write(
            scratch.join("repo/.git/worktrees/w/HEAD"),
            "ref: refs/heads/other\n",
        )
        .unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: ../repo/.git/worktrees/w\n").unwrap();
        assert_eq!(branch(&worktree).as_deref(), Some("other"));
        // Outside a repository, or with a `.git` file that names nothing, there is none.
        std::fs::write(worktree.join(".git"), "nonsense\n").unwrap();
        assert_eq!(branch(&worktree).as_deref(), None);
        let outside = scratch.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(head(&outside).is_none_or(|head| !head.file.starts_with(scratch)));
    }
}
