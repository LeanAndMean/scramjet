//! Telling the terminal that the agent waits for the user, Claude Code's
//! `preferredNotifChannel`: a desktop notification in the terminals that show one (iTerm2's OSC 9,
//! kitty's OSC 99, Ghostty's OSC 777) or the terminal's bell.

/// Where the notification goes, named as Claude Code's `preferredNotifChannel` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Channel {
    /// The terminal's own notification where it is known to have one, else none.
    Auto,
    ITerm2,
    ITerm2WithBell,
    Kitty,
    Ghostty,
    Bell,
    Disabled,
}

impl Channel {
    pub(super) const ALL: [Self; 7] = [
        Self::Auto,
        Self::Bell,
        Self::ITerm2,
        Self::ITerm2WithBell,
        Self::Kitty,
        Self::Ghostty,
        Self::Disabled,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ITerm2 => "iterm2",
            Self::ITerm2WithBell => "iterm2_with_bell",
            Self::Kitty => "kitty",
            Self::Ghostty => "ghostty",
            Self::Bell => "terminal_bell",
            Self::Disabled => "notifications_disabled",
        }
    }

    pub(super) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|channel| channel.name() == name)
    }

    /// The one `/settings` moves to from this one.
    pub(super) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|&channel| channel == self);
        Self::ALL[(at.expect("listed") + 1) % Self::ALL.len()]
    }

    /// As `/settings` shows it.
    pub(super) fn shown(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ITerm2 => "iTerm2",
            Self::ITerm2WithBell => "iTerm2 with bell",
            Self::Kitty => "kitty",
            Self::Ghostty => "Ghostty",
            Self::Bell => "terminal bell",
            Self::Disabled => "off",
        }
    }

    /// The channel used in the terminal `var` describes: `Auto` picks that terminal's own
    /// notification, as Claude Code's does, and there is none where it knows of none.
    pub(super) fn resolved(self, var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let is = |name: &str, value: &str| var(name).as_deref() == Some(value);
        match self {
            Self::Disabled => None,
            Self::Auto if is("TERM_PROGRAM", "iTerm.app") || is("LC_TERMINAL", "iTerm2") => {
                Some(Self::ITerm2)
            }
            Self::Auto
                if is("TERM_PROGRAM", "kitty")
                    || is("TERM", "xterm-kitty")
                    || var("KITTY_WINDOW_ID").is_some() =>
            {
                Some(Self::Kitty)
            }
            Self::Auto if is("TERM_PROGRAM", "ghostty") || is("TERM", "xterm-ghostty") => {
                Some(Self::Ghostty)
            }
            Self::Auto => None,
            channel => Some(channel),
        }
    }
}

/// What tells the terminal through `channel` (not `Auto` or `Disabled`), titled `title`; inside
/// tmux or screen, which `var` tells, the escape sequences are passed through to the terminal.
pub(super) fn sequence(
    channel: Channel,
    title: &str,
    message: &str,
    var: impl Fn(&str) -> Option<String>,
) -> String {
    const BELL: &str = "\x07";
    let (title, message) = (plain(title), plain(message));
    let notification = match channel {
        Channel::ITerm2 | Channel::ITerm2WithBell => format!("\x1b]9;{title}: {message}\x07"),
        // An id of its own so that the three parts make one notification.
        Channel::Kitty => {
            let id = std::process::id() % 10_000;
            format!(
                "\x1b]99;i={id}:d=0:p=title;{title}\x1b\\\x1b]99;i={id}:p=body;{message}\x1b\\\
                 \x1b]99;i={id}:d=1:a=focus;\x1b\\"
            )
        }
        Channel::Ghostty => format!("\x1b]777;notify;{};{message}\x07", title.replace(';', ",")),
        _ => String::new(),
    };
    let notification = match (var("TMUX").is_some(), var("STY").is_some()) {
        _ if notification.is_empty() => notification,
        (true, _) => format!(
            "\x1bPtmux;{}\x1b\\",
            notification.replace('\x1b', "\x1b\x1b")
        ),
        (_, true) => format!("\x1bP{}\x1b\\", notification.replace('\x1b', "\x1b\x1b")),
        _ => notification,
    };
    match channel {
        Channel::ITerm2WithBell | Channel::Bell => notification + BELL,
        _ => notification,
    }
}

/// `text` with its control characters as spaces, as Claude Code sends it.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            (vars.iter())
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn auto_picks_the_terminals_own_notification_and_none_elsewhere() {
        let auto = |vars| Channel::Auto.resolved(env(vars));
        assert_eq!(
            auto(&[("TERM_PROGRAM", "iTerm.app")]),
            Some(Channel::ITerm2)
        );
        assert_eq!(auto(&[("LC_TERMINAL", "iTerm2")]), Some(Channel::ITerm2));
        assert_eq!(auto(&[("KITTY_WINDOW_ID", "1")]), Some(Channel::Kitty));
        assert_eq!(auto(&[("TERM", "xterm-ghostty")]), Some(Channel::Ghostty));
        assert_eq!(auto(&[("WT_SESSION", "x")]), None);
        assert_eq!(auto(&[("TERM_PROGRAM", "Apple_Terminal")]), None);
        assert_eq!(Channel::Bell.resolved(env(&[])), Some(Channel::Bell));
        assert_eq!(Channel::Disabled.resolved(env(&[])), None);
    }

    #[test]
    fn each_channel_sends_its_terminals_sequence() {
        let none = env(&[]);
        let send = |channel| sequence(channel, "scramjet - project", "Waiting\x1b]0;x\x07", &none);
        let message = "Waiting ]0;x ";
        assert_eq!(
            send(Channel::ITerm2),
            format!("\x1b]9;scramjet - project: {message}\x07")
        );
        assert_eq!(
            send(Channel::ITerm2WithBell),
            format!("\x1b]9;scramjet - project: {message}\x07\x07")
        );
        assert_eq!(send(Channel::Bell), "\x07");
        let ghostty = sequence(Channel::Ghostty, "a;b", "m", &none);
        assert_eq!(ghostty, "\x1b]777;notify;a,b;m\x07");
        let kitty = send(Channel::Kitty);
        assert!(
            kitty.contains(":d=0:p=title;scramjet - project\x1b\\"),
            "{kitty:?}"
        );
        assert!(
            kitty.contains(&format!(":p=body;{message}\x1b\\")),
            "{kitty:?}"
        );
        assert!(kitty.ends_with(":d=1:a=focus;\x1b\\"), "{kitty:?}");
    }

    #[test]
    fn inside_tmux_the_notification_is_passed_through_and_the_bell_is_not() {
        let tmux = env(&[("TMUX", "/tmp/tmux-1/default,1,0")]);
        assert_eq!(
            sequence(Channel::ITerm2WithBell, "t", "m", &tmux),
            "\x1bPtmux;\x1b\x1b]9;t: m\x07\x1b\\\x07"
        );
        assert_eq!(sequence(Channel::Bell, "t", "m", &tmux), "\x07");
        let screen = env(&[("STY", "1.pts-0")]);
        assert_eq!(
            sequence(Channel::ITerm2, "t", "m", &screen),
            "\x1bP\x1b\x1b]9;t: m\x07\x1b\\"
        );
    }

    #[test]
    fn the_names_are_claude_codes_and_settings_cycles_through_them_all() {
        for channel in Channel::ALL {
            assert_eq!(Channel::named(channel.name()), Some(channel));
        }
        let mut seen = vec![Channel::Auto];
        while let next = seen.last().unwrap().next()
            && next != Channel::Auto
        {
            seen.push(next);
        }
        assert_eq!(seen, Channel::ALL);
    }
}
