use regress::{ExecBudget, MatchError, Regex};

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn budget(max_steps: u64) -> ExecBudget<'static> {
    ExecBudget {
        max_steps,
        poll_interval: 1 << 16,
        max_backtrack_entries: 1 << 20,
        poll: None,
    }
}

fn run(pattern: &str, flags: &str, text: &str) -> Result<Option<regress::Match>, MatchError> {
    let re = Regex::with_flags(pattern, flags).unwrap();
    let text = utf16(text);
    re.exec_utf16(&text, 0, false, budget(10_000_000))
}

#[test]
fn exponential_patterns_stop() {
    let attacks = [
        ("(a+)+$", "a".repeat(40) + "!"),
        ("(x+x+)+y", "x".repeat(5000)),
        ("(?:a|a)*b", "a".repeat(60)),
        ("((a*)*)*b", "a".repeat(60)),
        ("(a|aa)+$", "a".repeat(80) + "!"),
        (
            "^(\\w+\\s?)*$",
            "an attacker controlled sentence of words!".repeat(3),
        ),
        ("(?=(a+)+b)", "a".repeat(40)),
        ("(a+)\\1+$", "a".repeat(5000) + "!"),
    ];
    for (pattern, text) in attacks {
        let start = std::time::Instant::now();
        let result = run(pattern, "", &text);
        assert!(
            matches!(result, Err(MatchError::StepLimit | MatchError::StackLimit)),
            "{pattern}: {:?}",
            result.map(|m| m.is_some())
        );
        println!("{pattern}: {:?}", start.elapsed());
        assert!(
            start.elapsed().as_secs() < 10,
            "{pattern} took {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn ordinary_patterns_match() {
    let m = run("\\B(?=(\\d{3})+(?!\\d))", "", "1234567")
        .unwrap()
        .unwrap();
    assert_eq!(m.range(), 1..1);
    let m = run("(?<=\\$)\\d+", "", "cost $42").unwrap().unwrap();
    assert_eq!(m.range(), 6..8);
    let m = run("(a)\\1", "", "xaa").unwrap().unwrap();
    assert_eq!(m.range(), 1..3);
}

#[test]
fn poll_interrupts() {
    let re = Regex::new("(a+)+$").unwrap();
    let text = utf16(&("a".repeat(40) + "!"));
    let mut polls = 0;
    let mut poll = || {
        polls += 1;
        polls == 3
    };
    let result = re.exec_utf16(
        &text,
        0,
        false,
        ExecBudget {
            max_steps: u64::MAX,
            poll_interval: 1000,
            max_backtrack_entries: usize::MAX,
            poll: Some(&mut poll),
        },
    );
    assert_eq!(result.map(|m| m.is_some()), Err(MatchError::Interrupted));
    assert_eq!(polls, 3);
}

#[test]
fn backtrack_stack_is_bounded() {
    let re = Regex::new("(?:a|b)*c").unwrap();
    let text = utf16(&"ab".repeat(1 << 20));
    let result = re.exec_utf16(
        &text,
        0,
        false,
        ExecBudget {
            max_steps: u64::MAX,
            poll_interval: 1 << 12,
            max_backtrack_entries: 1 << 16,
            poll: None,
        },
    );
    assert_eq!(result.map(|m| m.is_some()), Err(MatchError::StackLimit));
}

#[test]
fn deep_nesting_is_a_syntax_error() {
    let pattern = "(".repeat(100_000) + &")".repeat(100_000);
    assert!(Regex::new(&pattern).is_err());
    let pattern = "[".repeat(100_000) + &"]".repeat(100_000);
    assert!(Regex::with_flags(&pattern, "v").is_err());
}

#[test]
fn long_alternation_compiles() {
    let pattern = "a|".repeat(100_000) + "b";
    let re = Regex::new(&pattern).unwrap();
    let text = utf16("xxb");
    let m = re
        .exec_utf16(&text, 0, false, budget(10_000_000))
        .unwrap()
        .unwrap();
    assert_eq!(m.range(), 2..3);
}

#[test]
fn lone_surrogates_and_sticky() {
    let text: Vec<u16> = vec![0x61, 0xD800, 0x62];
    let re = Regex::new(".").unwrap();
    let m = re
        .exec_utf16(&text, 1, true, budget(1000))
        .unwrap()
        .unwrap();
    assert_eq!(m.range(), 1..2);
    let re = Regex::with_flags("^.$", "u").unwrap();
    let pair = utf16("😀");
    assert!(
        re.exec_utf16(&pair, 0, false, budget(1000))
            .unwrap()
            .is_some()
    );
    let re = Regex::new("^.$").unwrap();
    assert!(
        re.exec_utf16(&pair, 0, false, budget(1000))
            .unwrap()
            .is_none()
    );
    let re = Regex::new("a").unwrap();
    let ba = utf16("ba");
    assert!(re.exec_utf16(&ba, 0, true, budget(1000)).unwrap().is_none());
}
