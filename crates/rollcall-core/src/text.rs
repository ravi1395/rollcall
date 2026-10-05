//! Text pipeline: §5.0 prompt filtering, §9 sanitising and secret scrubbing, §3.3 truncation,
//! and shell quoting for the §8.1 `c` / `Y` lines. Pure functions, no I/O.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

const MAX_GRAPHEMES: usize = 200;
const ELLIPSIS: &str = "…";
const REDACTED: &str = "[redacted]";

/// §5.0: a block starting with this header is agent-instruction boilerplate, not a prompt.
const AGENTS_HEADER: &str = "# AGENTS.md";
/// §5.0: harness-injected sections appended after real user text.
const INJECTED_MARKERS: [&str; 2] = ["<task-notification>", "<system-reminder>"];

/// §5.0 prompt text filtering for one text block: `None` when the block is not a human prompt
/// or nothing remains after cutting injected sections.
///
/// Leading whitespace is skipped before both start checks, so `"  # AGENTS.md"` is dropped too.
pub fn filter_prompt_block(block: &str) -> Option<String> {
    let head = block.trim_start();
    if head.starts_with('<') || head.starts_with(AGENTS_HEADER) {
        return None;
    }
    let cut = INJECTED_MARKERS
        .iter()
        .filter_map(|marker| head.find(marker))
        .min()
        .unwrap_or(head.len());
    let kept = head[..cut].trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// ESC-introduced sequences: CSI (`ESC [ params final`), OSC (`ESC ] … BEL | ST`), otherwise
/// ESC plus one character. Each alternative tolerates truncation, so a dangling sequence is
/// dropped rather than left half-visible, and none consumes a following ESC.
static ESCAPE_SEQUENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)\x1b\[[0-?]*[ -/]*[@-~]?|\x1b\][^\x07\x1b\x{9c}]*(?:\x07|\x1b\\|\x{9c})?|\x1b[^\x1b]?",
    )
    .expect("valid escape-sequence regex")
});

/// The §9 format/invisible characters (bidi controls, zero-width, BOM, …) that survive
/// `char::is_control`.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{AD}'
            | '\u{61C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// §9 sanitising: drops escape sequences, C0/DEL/C1 controls and invisible format characters;
/// newline, tab, U+2028 and U+2029 become a space.
pub fn sanitise(text: &str) -> String {
    ESCAPE_SEQUENCE
        .replace_all(text, "")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\t' | '\u{2028}' | '\u{2029}' => Some(' '),
            c if c.is_control() || is_invisible(c) => None,
            c => Some(c),
        })
        .collect()
}

/// §9 secret patterns, verbatim.
const SECRET_PATTERNS: [&str; 14] = [
    r"sk-[A-Za-z0-9_-]{16,}",
    r"(sk|rk)_(live|test)_[A-Za-z0-9]{10,}",
    r"AKIA[0-9A-Z]{16}",
    r"AIza[0-9A-Za-z_-]{35}",
    r"gh[pousr]_[A-Za-z0-9]{36,}",
    r"github_pat_[A-Za-z0-9_]{20,}",
    r"npm_[A-Za-z0-9]{30,}",
    r"xox[abprs]-[A-Za-z0-9-]{10,}",
    r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
    r"Bearer\s+\S+",
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
    r"(?i)\b(password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S+",
    r"[A-Za-z][A-Za-z0-9+.-]*://[^/\s:@]+:[^@\s/]+@",
    // Email must stay last: `EMAIL_PATTERN` is the final index.
    r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
];

/// Index of the email pattern in `SECRET_PATTERNS`; it applies only with `redact_emails`.
const EMAIL_PATTERN: usize = SECRET_PATTERNS.len() - 1;

static SECRETS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    SECRET_PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("valid secret regex"))
        .collect()
});

/// §9 secret scrubbing: every match becomes `[redacted]`; emails only when `redact_emails`.
pub fn scrub(text: &str, redact_emails: bool) -> String {
    let active = if redact_emails {
        &SECRETS[..]
    } else {
        &SECRETS[..EMAIL_PATTERN]
    };
    active.iter().fold(text.to_owned(), |acc, re| {
        re.replace_all(&acc, REDACTED).into_owned()
    })
}

/// Longest `&…;` reference tried: `&#x10FFFF;` is 10 bytes.
const MAX_ENTITY: usize = 12;

/// §8.2: decodes `&lt; &gt; &amp; &quot;` and numeric `&#N;` / `&#xH;` references in one pass.
/// A numeric reference that is not a Unicode scalar value, and any other `&…;`, stays as written.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let (c, len) = entity(rest).unwrap_or(('&', 1));
        out.push(c);
        rest = &rest[len..];
    }
    out.push_str(rest);
    out
}

/// The character and byte length of the reference at the start of `s` (which starts with `&`).
fn entity(s: &str) -> Option<(char, usize)> {
    let end = s.bytes().take(MAX_ENTITY).position(|b| b == b';')?;
    let c = match &s[1..end] {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "quot" => '"',
        body => {
            let digits = body.strip_prefix('#')?;
            let (radix, digits) = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => (16, hex),
                None => (10, digits),
            };
            // `from_str_radix` alone would accept a leading `+`.
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return None;
            }
            char::from_u32(u32::from_str_radix(digits, radix).ok()?)?
        }
    };
    Some((c, end + 1))
}

/// Number of extended grapheme clusters.
pub fn grapheme_len(text: &str) -> usize {
    text.graphemes(true).count()
}

/// §3.3: at most 200 grapheme clusters including a trailing `…`. Text already within the
/// limit is returned unchanged, even if it ends in `…`.
pub fn truncate200(text: &str) -> String {
    let mut starts = text.grapheme_indices(true).map(|(i, _)| i);
    // Start of the last grapheme that fits alongside the ellipsis (the 200th).
    let Some(cut) = starts.nth(MAX_GRAPHEMES - 1) else {
        return text.to_owned();
    };
    if starts.next().is_none() {
        return text.to_owned();
    }
    format!("{}{ELLIPSIS}", &text[..cut])
}

/// The full display pipeline for a title or prompt: sanitise, then scrub, then truncate.
pub fn render_text(text: &str, redact_emails: bool) -> String {
    truncate200(&scrub(&sanitise(text), redact_emails))
}

/// POSIX single-quoting: `'` becomes `'\''`, everything else is literal.
///
/// Non-UTF-8 path bytes cannot be represented in the returned `String`; they become U+FFFD
/// (lossy), so the quoted text is safe to paste but would name a different path. Callers that
/// must round-trip such paths must not use this.
pub fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::process::Command;

    use super::*;

    // ---- filter_prompt_block ----

    #[test]
    fn prompt_block_starting_with_angle_bracket_is_dropped() {
        assert_eq!(
            filter_prompt_block("<command-name>/clear</command-name>"),
            None
        );
    }

    #[test]
    fn prompt_block_with_leading_whitespace_then_angle_bracket_is_dropped() {
        assert_eq!(filter_prompt_block("  \n\t<local-command-stdout>x"), None);
    }

    #[test]
    fn prompt_block_with_inner_angle_bracket_is_kept() {
        assert_eq!(
            filter_prompt_block("use Vec<u8> here").as_deref(),
            Some("use Vec<u8> here")
        );
    }

    #[test]
    fn agents_md_block_is_dropped() {
        assert_eq!(
            filter_prompt_block("# AGENTS.md instructions for /repo\nbody"),
            None
        );
    }

    #[test]
    fn agents_md_mention_mid_block_is_kept() {
        assert_eq!(
            filter_prompt_block("please read # AGENTS.md").as_deref(),
            Some("please read # AGENTS.md")
        );
    }

    #[test]
    fn prompt_is_cut_at_system_reminder() {
        assert_eq!(
            filter_prompt_block("fix the bug\n<system-reminder>ignore me</system-reminder>")
                .as_deref(),
            Some("fix the bug")
        );
    }

    #[test]
    fn prompt_is_cut_at_task_notification() {
        assert_eq!(
            filter_prompt_block("run tests <task-notification>done</task-notification>").as_deref(),
            Some("run tests")
        );
    }

    #[test]
    fn prompt_is_cut_at_the_earlier_of_the_two_markers() {
        let block = "a <task-notification>x <system-reminder>y";
        assert_eq!(filter_prompt_block(block).as_deref(), Some("a"));
        let block = "a <system-reminder>x <task-notification>y";
        assert_eq!(filter_prompt_block(block).as_deref(), Some("a"));
    }

    #[test]
    fn prompt_that_is_only_an_injected_section_after_whitespace_is_none() {
        assert_eq!(filter_prompt_block("   <system-reminder>x"), None);
    }

    #[test]
    fn whitespace_only_or_empty_block_is_none() {
        assert_eq!(filter_prompt_block(""), None);
        assert_eq!(filter_prompt_block(" \n\t "), None);
    }

    #[test]
    fn kept_prompt_is_trimmed() {
        assert_eq!(filter_prompt_block("  hello \n").as_deref(), Some("hello"));
    }

    // ---- sanitise ----

    #[test]
    fn sanitise_removes_csi_sequences() {
        assert_eq!(sanitise("a\x1b[2Jb"), "ab");
        assert_eq!(sanitise("\x1b[31;1mred\x1b[0m"), "red");
    }

    #[test]
    fn sanitise_removes_osc_terminated_by_bel_or_st() {
        assert_eq!(sanitise("a\x1b]0;window title\x07b"), "ab");
        assert_eq!(sanitise("a\x1b]8;;http://x\x1b\\b"), "ab");
    }

    #[test]
    fn sanitise_removes_two_byte_escape_sequences() {
        assert_eq!(sanitise("a\x1bcb"), "ab");
        assert_eq!(sanitise("a\x1b=b"), "ab");
    }

    #[test]
    fn sanitise_drops_dangling_and_doubled_escapes() {
        assert_eq!(sanitise("a\x1b"), "a");
        assert_eq!(sanitise("a\x1b\x1b[2Jb"), "ab");
        assert_eq!(sanitise("a\x1b[1;"), "a");
        assert_eq!(sanitise("a\x1b]0;unterminated"), "a");
    }

    #[test]
    fn sanitise_removes_every_other_c0_control_and_del() {
        for code in (0u8..0x20).chain([0x7f]) {
            if matches!(code, 0x1b | b'\n' | b'\t') {
                continue;
            }
            let input = format!("a{}b", code as char);
            assert_eq!(sanitise(&input), "ab", "U+{code:04X}");
        }
    }

    #[test]
    fn sanitise_removes_c1_controls_including_csi() {
        assert_eq!(sanitise("a\u{9b}b"), "ab");
        assert_eq!(sanitise("a\u{80}b"), "ab");
        assert_eq!(sanitise("a\u{9f}b"), "ab");
    }

    #[test]
    fn sanitise_removes_each_listed_format_character_and_range_endpoint() {
        let listed: Vec<char> = [
            0xAD..=0xAD,
            0x61C..=0x61C,
            0x180E..=0x180E,
            0x200B..=0x200F,
            0x202A..=0x202E,
            0x2060..=0x2064,
            0x2066..=0x2069,
            0xFEFF..=0xFEFF,
            0xFFF9..=0xFFFB,
        ]
        .into_iter()
        .flatten()
        .map(|c| char::from_u32(c).unwrap())
        .collect();
        assert_eq!(listed.len(), 1 + 1 + 1 + 5 + 5 + 5 + 4 + 1 + 3);
        for c in listed {
            assert_eq!(sanitise(&format!("a{c}b")), "ab", "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn sanitise_removes_rtl_override_and_zero_width_space() {
        assert_eq!(sanitise("exe\u{202E}gpj.txt"), "exegpj.txt");
        assert_eq!(sanitise("pass\u{200B}word"), "password");
    }

    #[test]
    fn sanitise_keeps_characters_just_outside_the_listed_ranges() {
        for c in [
            '\u{A0}', '\u{AC}', '\u{AE}', '\u{61B}', '\u{61D}', '\u{180D}', '\u{200A}', '\u{2010}',
            '\u{202F}', '\u{205F}', '\u{2065}', '\u{206A}', '\u{FEFE}', '\u{FFF8}', '\u{FFFC}',
            'é', '日',
        ] {
            assert_eq!(
                sanitise(&format!("a{c}b")),
                format!("a{c}b"),
                "U+{:04X}",
                c as u32
            );
        }
    }

    #[test]
    fn sanitise_turns_newline_and_tab_into_spaces() {
        assert_eq!(sanitise("a\nb\tc"), "a b c");
    }

    #[test]
    fn sanitise_turns_line_and_paragraph_separators_into_spaces() {
        for sep in ['\u{2028}', '\u{2029}'] {
            assert_eq!(sanitise(&format!("a{sep}b")), "a b", "U+{:04X}", sep as u32);
            assert_eq!(
                sanitise(&format!("{sep}ab{sep}")),
                " ab ",
                "U+{:04X}",
                sep as u32
            );
            assert_eq!(
                sanitise(&format!("a{sep}{sep}b")),
                "a  b",
                "U+{:04X}",
                sep as u32
            );
        }
        assert_eq!(sanitise("a\u{2028}b\u{2029}c"), "a b c");
    }

    #[test]
    fn sanitise_leaves_plain_text_alone() {
        assert_eq!(sanitise("plain text, ünïcode ✓"), "plain text, ünïcode ✓");
    }

    // ---- scrub ----

    /// `positive` must be fully replaced; `negative` must pass through untouched.
    fn assert_scrubs(positive: &str, negative: &str) {
        assert_eq!(scrub(positive, false), REDACTED, "positive {positive:?}");
        assert_eq!(scrub(negative, false), negative, "negative {negative:?}");
    }

    #[test]
    fn scrub_sk_keys() {
        assert_scrubs("sk-abcdefghijklmnop", "sk-abcdefghijklmno");
        assert_scrubs("sk-ant_api03-AbCdEf012345_xyz", "task-abcdefgh");
    }

    #[test]
    fn scrub_stripe_style_keys() {
        assert_scrubs("sk_live_abcdefghij", "sk_live_abcdefghi");
        assert_scrubs("rk_test_ABCDEFGHIJ0123", "rk_prod_abcdefghij");
    }

    #[test]
    fn scrub_aws_access_key_ids() {
        assert_scrubs("AKIAIOSFODNN7EXAMPLE", "AKIAIOSFODNN7EXAMPL");
        assert_scrubs("AKIA0123456789ABCDEF", "AKIAiosfodnn7exampleX");
    }

    #[test]
    fn scrub_google_api_keys() {
        let key = format!("AIza{}", "a".repeat(35));
        let short = format!("AIza{}", "a".repeat(34));
        assert_scrubs(&key, &short);
    }

    #[test]
    fn scrub_github_tokens() {
        for kind in ["p", "o", "u", "s", "r"] {
            let tok = format!("gh{kind}_{}", "A".repeat(36));
            assert_eq!(scrub(&tok, false), REDACTED, "gh{kind}_");
        }
        assert_scrubs(
            &format!("ghp_{}", "a".repeat(36)),
            &format!("ghp_{}", "a".repeat(35)),
        );
        assert_eq!(
            scrub(&format!("ghx_{}", "a".repeat(36)), false),
            format!("ghx_{}", "a".repeat(36))
        );
    }

    #[test]
    fn scrub_github_fine_grained_pats() {
        assert_scrubs(
            &format!("github_pat_{}", "A1_".repeat(7)),
            &format!("github_pat_{}", "a".repeat(19)),
        );
    }

    #[test]
    fn scrub_npm_tokens() {
        assert_scrubs(
            &format!("npm_{}", "a".repeat(30)),
            &format!("npm_{}", "a".repeat(29)),
        );
    }

    #[test]
    fn scrub_slack_tokens() {
        assert_scrubs("xoxb-1234567890", "xoxb-123456789");
        assert_eq!(scrub("xoxz-1234567890", false), "xoxz-1234567890");
        for kind in ["a", "b", "p", "r", "s"] {
            assert_eq!(scrub(&format!("xox{kind}-1234567890"), false), REDACTED);
        }
    }

    #[test]
    fn scrub_jwts() {
        assert_scrubs(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4",
            "eyJhbGciOiJIUzI1NiJ9.notajwt.sig",
        );
    }

    #[test]
    fn scrub_bearer_credentials() {
        assert_scrubs("Bearer abc.def-123", "Bearer");
        assert_eq!(
            scrub("Authorization: Bearer xyz", false),
            "Authorization: [redacted]"
        );
        // The pattern is case-sensitive as written in §9.
        assert_eq!(scrub("bearer abc", false), "bearer abc");
    }

    #[test]
    fn scrub_private_key_headers() {
        assert_scrubs(
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----BEGIN PUBLIC KEY-----",
        );
        assert_scrubs("-----BEGIN PRIVATE KEY-----", "BEGIN PRIVATE KEY");
        assert_scrubs(
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "-----END PRIVATE KEY-----",
        );
    }

    #[test]
    fn scrub_assignment_style_secrets() {
        assert_eq!(scrub("password=hunter2", false), REDACTED);
        assert_eq!(scrub("PASSWD : hunter2", false), REDACTED);
        assert_eq!(scrub("my secret: s3cr3t rest", false), "my [redacted] rest");
        assert_eq!(scrub("token=abc", false), REDACTED);
        assert_eq!(scrub("API_KEY=abc", false), REDACTED);
        assert_eq!(scrub("api-key: abc", false), REDACTED);
        assert_eq!(scrub("apikey=abc", false), REDACTED);
    }

    #[test]
    fn scrub_assignment_near_misses_are_kept() {
        for keep in [
            "password reset flow",
            "tokenizer=bpe",
            "mytoken=abc",
            "secretive: yes",
            "password=",
            "the token",
        ] {
            assert_eq!(scrub(keep, false), keep, "{keep:?}");
        }
    }

    #[test]
    fn scrub_credentials_in_urls() {
        assert_eq!(
            scrub("git clone https://user:pw@example.com/r.git", false),
            "git clone [redacted]example.com/r.git"
        );
        for keep in [
            "https://example.com:8080/path",
            "https://user@example.com/",
            "see https://example.com/a@b:c",
        ] {
            assert_eq!(scrub(keep, false), keep, "{keep:?}");
        }
    }

    #[test]
    fn scrub_emails_only_with_the_flag() {
        let text = "mail ravi.c+x@example.co.uk now";
        assert_eq!(scrub(text, false), text);
        assert_eq!(scrub(text, true), "mail [redacted] now");
    }

    #[test]
    fn scrub_email_near_misses_are_kept_even_with_the_flag() {
        for keep in ["user@", "@example.com", "a@b", "a@b.c", "not an email"] {
            assert_eq!(scrub(keep, true), keep, "{keep:?}");
        }
    }

    #[test]
    fn scrub_redacts_every_match_in_a_string() {
        assert_eq!(
            scrub("a sk-abcdefghijklmnop b AKIAIOSFODNN7EXAMPLE c", false),
            "a [redacted] b [redacted] c"
        );
    }

    #[test]
    fn scrub_leaves_plain_text_alone() {
        assert_eq!(
            scrub("fix the flaky e2e test", true),
            "fix the flaky e2e test"
        );
    }

    // ---- truncate200 / grapheme_len ----

    #[test]
    fn grapheme_len_counts_clusters_not_chars() {
        assert_eq!(grapheme_len(""), 0);
        assert_eq!(grapheme_len("abc"), 3);
        assert_eq!(grapheme_len("e\u{301}"), 1);
        assert_eq!(grapheme_len("👨‍👩‍👧‍👦"), 1);
    }

    #[test]
    fn text_of_201_graphemes_becomes_200_including_ellipsis() {
        let out = truncate200(&"a".repeat(201));
        assert_eq!(grapheme_len(&out), 200);
        assert_eq!(out, format!("{}…", "a".repeat(199)));
    }

    #[test]
    fn text_of_200_graphemes_is_unchanged() {
        let text = "b".repeat(200);
        assert_eq!(truncate200(&text), text);
    }

    #[test]
    fn text_shorter_than_the_limit_and_empty_are_unchanged() {
        assert_eq!(truncate200(""), "");
        assert_eq!(truncate200("short"), "short");
    }

    #[test]
    fn text_of_200_graphemes_ending_in_ellipsis_is_unchanged() {
        let text = format!("{}…", "c".repeat(199));
        assert_eq!(grapheme_len(&text), 200);
        assert_eq!(truncate200(&text), text);
    }

    #[test]
    fn text_of_201_graphemes_ending_in_ellipsis_is_cut_again() {
        let text = format!("{}…", "c".repeat(200));
        assert_eq!(truncate200(&text), format!("{}…", "c".repeat(199)));
    }

    #[test]
    fn zwj_family_emoji_at_the_boundary_is_never_split() {
        let family = "👨‍👩‍👧‍👦";
        // Family is grapheme #200: kept whole only when it is the last of 200 clusters.
        let fits = format!("{}{family}", "a".repeat(199));
        assert_eq!(truncate200(&fits), fits);

        // Family is the 199th cluster of 201: kept whole, followed by the ellipsis.
        let over = format!("{}{family}zz", "a".repeat(198));
        assert_eq!(truncate200(&over), format!("{}{family}…", "a".repeat(198)));

        // Family is the 200th cluster of a longer string: it is dropped whole for the ellipsis.
        let over = format!("{}{family}zz", "a".repeat(199));
        let out = truncate200(&over);
        assert_eq!(out, format!("{}…", "a".repeat(199)));
    }

    #[test]
    fn combining_marks_stay_with_their_base_at_the_boundary() {
        let text = format!("{}e\u{301}zz", "a".repeat(198));
        assert_eq!(truncate200(&text), format!("{}e\u{301}…", "a".repeat(198)));
    }

    // ---- decode_entities ----

    #[test]
    fn decode_entities_decodes_the_named_five() {
        assert_eq!(decode_entities("&lt;command-name&gt;"), "<command-name>");
        assert_eq!(decode_entities("&gt;"), ">");
        assert_eq!(decode_entities("&amp;"), "&");
        assert_eq!(decode_entities("&quot;"), "\"");
        assert_eq!(decode_entities("&#39;"), "'");
    }

    #[test]
    fn decode_entities_decodes_numeric_references() {
        assert_eq!(decode_entities("&#65;"), "A");
        assert_eq!(decode_entities("&#x41;"), "A");
        assert_eq!(decode_entities("&#X41;"), "A");
        assert_eq!(decode_entities("&#x1F600;"), "\u{1F600}");
    }

    #[test]
    fn decode_entities_is_one_pass() {
        assert_eq!(decode_entities("&amp;lt;"), "&lt;");
        assert_eq!(decode_entities("&amp;amp;"), "&amp;");
    }

    #[test]
    fn decode_entities_leaves_everything_else_as_written() {
        for raw in [
            "&nbsp;",
            "&#xD800;",
            "&#x110000;",
            "&#;",
            "&#xZZ;",
            "&#x;",
            "&#+65;",
            "&#99999999999999999999;",
            "&lt",
            "&",
            "a & b",
            "&&lt;",
        ] {
            let want = if raw == "&&lt;" { "&<" } else { raw };
            assert_eq!(decode_entities(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn decode_entities_keeps_surrounding_text_and_multibyte_chars() {
        assert_eq!(
            decode_entities("é &lt; ü &#233; &nbsp;é"),
            "é < ü é &nbsp;é"
        );
    }

    // ---- render_text ----

    #[test]
    fn render_text_sanitises_before_scrubbing() {
        // A zero-width space must not let a secret slip past the pattern.
        assert_eq!(render_text("sk-abcdefgh\u{200B}ijklmnop", false), REDACTED);
    }

    #[test]
    fn render_text_scrubs_before_truncating() {
        let text = format!("{} sk-abcdefghijklmnop", "a".repeat(185));
        assert_eq!(
            render_text(&text, false),
            format!("{} {REDACTED}", "a".repeat(185))
        );
        let text = format!("{}sk-abcdefghijklmnop{}", "a".repeat(195), "b".repeat(10));
        let out = render_text(&text, false);
        assert!(!out.contains("sk-"));
        assert_eq!(grapheme_len(&out), 200);
    }

    #[test]
    fn render_text_email_flag_is_passed_through() {
        assert_eq!(render_text("a@example.com", false), "a@example.com");
        assert_eq!(render_text("a@example.com", true), REDACTED);
    }

    #[test]
    fn render_text_strips_terminal_injection() {
        assert_eq!(
            render_text("x\x1b[2J\u{9b}\u{202E}\u{200B}y\nz", false),
            "xy z"
        );
    }

    // ---- shell_quote ----

    /// Hostile paths from spec §10.2: space, `$`, backtick, `;`, `'`, newline.
    const HOSTILE_PATHS: [&str; 8] = [
        "/tmp/a b",
        "/tmp/a$HOME",
        "/tmp/a`echo pwned`",
        "/tmp/a;echo pwned",
        "/tmp/it's",
        "/tmp/a\nb",
        "/tmp/$(echo pwned)",
        "/tmp/'; echo pwned; '",
    ];

    fn sh_prints(quoted: &str) -> Vec<u8> {
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s' {quoted}"))
            .output()
            .expect("spawn /bin/sh");
        assert!(out.status.success(), "sh failed for {quoted:?}: {out:?}");
        out.stdout
    }

    #[test]
    fn shell_quote_round_trips_through_sh_for_every_hostile_path() {
        for raw in HOSTILE_PATHS {
            let quoted = shell_quote(Path::new(raw));
            assert_eq!(
                sh_prints(&quoted),
                raw.as_bytes(),
                "{raw:?} quoted as {quoted}"
            );
        }
    }

    #[test]
    fn shell_quote_shape_for_cd_lines() {
        assert_eq!(shell_quote(Path::new("/tmp/x")), "'/tmp/x'");
        assert_eq!(shell_quote(Path::new("/tmp/it's")), r"'/tmp/it'\''s'");
        assert_eq!(shell_quote(Path::new("")), "''");
        let line = format!("cd -- {}", shell_quote(Path::new("/tmp/a b")));
        assert_eq!(line, "cd -- '/tmp/a b'");
    }

    #[test]
    fn shell_quote_cd_line_lands_in_the_directory() {
        let dir = std::env::temp_dir().join("rollcall text quote 'x' $HOME");
        std::fs::create_dir_all(&dir).unwrap();
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("cd -- {} && pwd -P", shell_quote(&dir)))
            .output()
            .unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert!(out.status.success(), "{out:?}");
        let landed = String::from_utf8(out.stdout).unwrap();
        assert!(
            landed.trim_end().ends_with("rollcall text quote 'x' $HOME"),
            "{landed}"
        );
    }

    #[test]
    fn shell_quote_replaces_non_utf8_bytes_with_the_replacement_character() {
        let path = Path::new(OsStr::from_bytes(b"/tmp/a\xffb"));
        assert_eq!(shell_quote(path), "'/tmp/a\u{FFFD}b'");
    }

    #[test]
    fn breaker2_sanitise_maps_each_separator_to_one_space_next_to_other_classes() {
        for sep in ['\u{2028}', '\u{2029}'] {
            let tag = format!("U+{:04X}", sep as u32);
            for (input, want) in [
                (format!("{sep}"), " "),
                (format!("{sep}{sep}{sep}"), "   "),
                (format!("a\n{sep}\tb"), "a   b"),
                (format!("a\u{200b}{sep}\u{200b}b"), "a b"),
                (format!("a\u{7f}{sep}\u{9b}b"), "a b"),
                (format!("a\u{202e}{sep}\u{feff}b"), "a b"),
                (format!("a\x1b[2J{sep}\x1b[0mb"), "a b"),
                (format!("a\x1b]0;t\x07{sep}b"), "a b"),
                (format!("{sep}\u{2028}\u{2029}{sep}"), "    "),
                (format!("x{sep}y{sep}"), "x y "),
            ] {
                let out = sanitise(&input);
                assert_eq!(out, want, "{tag}: {input:?}");
                assert!(!out.contains(['\u{2028}', '\u{2029}']), "{tag}: {out:?}");
            }
        }
        // Characters next to the separators that are not on the §9 list stay as they are.
        assert_eq!(
            sanitise("\u{2027}\u{2028}\u{202a}\u{2029}\u{2030}"),
            "\u{2027}  \u{2030}"
        );
        assert_eq!(sanitise("\u{85}\u{2028}"), " ");
    }
}
