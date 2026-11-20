//! Regex and wildcard topic matching for contract events.
//!
//! Provides [`assert_emitted_matches!`] and [`RegexEventMatcher`] allowing tests
//! to assert emitted events using glob wildcards (`*`, `?`) and regular expressions
//! across event topics and data payload fields.

use soroban_sdk::xdr::{ContractEvent, ScVal};
use soroban_sdk::{Address, Env, TryFromVal, Val};

use crate::events::{event_data, event_topics, EventSource};
use crate::payload::read_as;

/// Evaluates a glob pattern with `*` (0 or more chars) and `?` (1 char).
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let pat = pattern.as_bytes();
    let txt = text.as_bytes();
    let mut p = 0;
    let mut t = 0;
    let mut star_p = None;
    let mut star_t = 0;

    while t < txt.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = Some(p);
            p += 1;
            star_t = t;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }

    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }

    p == pat.len()
}

/// A lightweight regex evaluator supporting `^`, `$`, `|`, `()`, `*`, `+`, `?`,
/// `.`, and character classes like `\d`, `\w`.
pub fn regex_matches(pattern: &str, text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let pat_chars: Vec<char> = pattern.chars().collect();

    let anchored_start = pat_chars.first() == Some(&'^');
    let anchored_end = pat_chars.last() == Some(&'$');

    let start_idx = if anchored_start { 1 } else { 0 };
    let end_idx = if anchored_end {
        pat_chars.len().saturating_sub(1)
    } else {
        pat_chars.len()
    };

    if start_idx > end_idx {
        return text.is_empty();
    }

    let core_pat = &pat_chars[start_idx..end_idx];

    if anchored_start {
        match_regex_here(core_pat, &chars, 0, anchored_end)
    } else {
        for start in 0..=chars.len() {
            if match_regex_here(core_pat, &chars, start, anchored_end) {
                return true;
            }
        }
        false
    }
}

fn match_regex_here(pat: &[char], txt: &[char], txt_idx: usize, anchored_end: bool) -> bool {
    if pat.is_empty() {
        return if anchored_end {
            txt_idx == txt.len()
        } else {
            true
        };
    }

    // Check for top-level alternation outside parentheses: `A|B`
    let mut paren_depth = 0;
    let mut alt_idx = None;
    for (i, &c) in pat.iter().enumerate() {
        match c {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '|' if paren_depth == 0 => {
                alt_idx = Some(i);
                break;
            }
            _ => {}
        }
    }

    if let Some(pipe) = alt_idx {
        let left = &pat[..pipe];
        let right = &pat[pipe + 1..];
        return match_regex_here(left, txt, txt_idx, anchored_end)
            || match_regex_here(right, txt, txt_idx, anchored_end);
    }

    // Consume first atom (char, group, class)
    let (atom_matched, atom_len, pat_consumed) = match pat[0] {
        '(' => {
            // Find matching closing paren
            let mut depth = 1;
            let mut close_idx = 0;
            for (i, &c) in pat[1..].iter().enumerate() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 {
                        close_idx = i + 1;
                        break;
                    }
                }
            }
            if close_idx == 0 {
                return false; // Malformed regex
            }
            let group_pat = &pat[1..close_idx];
            // Evaluate sub-pattern at current text position
            let mut possible_advances = Vec::new();
            for advance in 0..=(txt.len() - txt_idx) {
                if match_regex_here(group_pat, &txt[..txt_idx + advance], txt_idx, true) {
                    possible_advances.push(advance);
                }
            }
            // For group with quantifier or single match:
            let next_pat_idx = close_idx + 1;
            let quantifier = pat.get(next_pat_idx).copied();
            return match quantifier {
                Some('*') => {
                    let rest_pat = &pat[next_pat_idx + 1..];
                    if match_regex_here(rest_pat, txt, txt_idx, anchored_end) {
                        return true;
                    }
                    for &adv in &possible_advances {
                        if adv > 0 && match_regex_here(pat, txt, txt_idx + adv, anchored_end) {
                            return true;
                        }
                    }
                    false
                }
                Some('+') => {
                    let rest_pat = &pat[next_pat_idx + 1..];
                    for &adv in &possible_advances {
                        if adv > 0
                            && (match_regex_here(rest_pat, txt, txt_idx + adv, anchored_end)
                                || match_regex_here(pat, txt, txt_idx + adv, anchored_end))
                        {
                            return true;
                        }
                    }
                    false
                }
                Some('?') => {
                    let rest_pat = &pat[next_pat_idx + 1..];
                    if match_regex_here(rest_pat, txt, txt_idx, anchored_end) {
                        return true;
                    }
                    for &adv in &possible_advances {
                        if match_regex_here(rest_pat, txt, txt_idx + adv, anchored_end) {
                            return true;
                        }
                    }
                    false
                }
                _ => {
                    let rest_pat = &pat[next_pat_idx..];
                    for &adv in &possible_advances {
                        if match_regex_here(rest_pat, txt, txt_idx + adv, anchored_end) {
                            return true;
                        }
                    }
                    false
                }
            };
        }
        '\\' if pat.len() > 1 => {
            let esc = pat[1];
            let matched = if txt_idx < txt.len() {
                let ch = txt[txt_idx];
                match esc {
                    'd' => ch.is_ascii_digit(),
                    'w' => ch.is_alphanumeric() || ch == '_',
                    's' => ch.is_whitespace(),
                    other => ch == other,
                }
            } else {
                false
            };
            (matched, 1, 2)
        }
        '.' => {
            let matched = txt_idx < txt.len();
            (matched, 1, 1)
        }
        lit => {
            let matched = txt_idx < txt.len() && txt[txt_idx] == lit;
            (matched, 1, 1)
        }
    };

    let next_pat_idx = pat_consumed;
    let quantifier = pat.get(next_pat_idx).copied();

    match quantifier {
        Some('*') => {
            let rest_pat = &pat[next_pat_idx + 1..];
            if match_regex_here(rest_pat, txt, txt_idx, anchored_end) {
                return true;
            }
            if atom_matched && txt_idx < txt.len() {
                return match_regex_here(pat, txt, txt_idx + atom_len, anchored_end);
            }
            false
        }
        Some('+') => {
            let rest_pat = &pat[next_pat_idx + 1..];
            if atom_matched && txt_idx < txt.len() {
                if match_regex_here(rest_pat, txt, txt_idx + atom_len, anchored_end) {
                    return true;
                }
                return match_regex_here(pat, txt, txt_idx + atom_len, anchored_end);
            }
            false
        }
        Some('?') => {
            let rest_pat = &pat[next_pat_idx + 1..];
            if match_regex_here(rest_pat, txt, txt_idx, anchored_end) {
                return true;
            }
            if atom_matched && txt_idx < txt.len() {
                return match_regex_here(rest_pat, txt, txt_idx + atom_len, anchored_end);
            }
            false
        }
        _ => {
            if atom_matched && txt_idx < txt.len() {
                let rest_pat = &pat[next_pat_idx..];
                match_regex_here(rest_pat, txt, txt_idx + atom_len, anchored_end)
            } else {
                false
            }
        }
    }
}

/// Dispatches pattern matching using regex if regular expression syntax is detected,
/// or glob matching otherwise.
pub fn matches_topic_pattern(pattern: &str, topic: &str) -> bool {
    let is_regex = pattern.starts_with('^')
        || pattern.ends_with('$')
        || pattern.contains('|')
        || pattern.contains('(')
        || pattern.contains(r"\d")
        || pattern.contains(r"\w");

    if is_regex {
        regex_matches(pattern, topic)
    } else {
        glob_matches(pattern, topic)
    }
}

/// Matches an event's topics against `pattern`.
///
/// An event matches if any individual topic matches, or if the colon-delimited
/// joined topic string (e.g. `"Order:Created:104"`) matches.
pub fn event_matches_topic_pattern(event: &ContractEvent, pattern: &str) -> bool {
    let topics = event_topics(event);
    if topics.is_empty() {
        return matches_topic_pattern(pattern, "");
    }

    // 1. Check individual topics
    for topic in &topics {
        if matches_topic_pattern(pattern, topic) {
            return true;
        }
    }

    // 2. Check joined colon-delimited topics
    let joined = topics.join(":");
    matches_topic_pattern(pattern, &joined)
}

type DataPredicate<'a> = Box<dyn Fn(&Env, &ScVal) -> Result<(), String> + 'a>;

/// Ergonomic matcher for contract events supporting wildcard and regex topic matching.
pub struct RegexEventMatcher<'a> {
    env: &'a Env,
    contract: Option<Address>,
    topic_pattern: Option<String>,
    events: Option<&'a [ContractEvent]>,
    data_predicates: Vec<DataPredicate<'a>>,
}

impl<'a> RegexEventMatcher<'a> {
    pub fn new(source: &'a impl EventSource) -> Self {
        let matcher = source.event_matcher();
        Self {
            env: matcher.env(),
            contract: None,
            topic_pattern: None,
            events: matcher.events_slice(),
            data_predicates: Vec::new(),
        }
    }

    pub fn from_contract(&mut self, contract: &Address) -> &mut Self {
        self.contract = Some(contract.clone());
        self
    }

    pub fn with_topic_pattern(&mut self, pattern: impl Into<String>) -> &mut Self {
        self.topic_pattern = Some(pattern.into());
        self
    }

    pub fn with_data_field<T>(&mut self, field_name: &'static str, expected: T) -> &mut Self
    where
        T: TryFromVal<Env, Val> + PartialEq + std::fmt::Debug + Clone + 'a,
    {
        self.data_predicates.push(Box::new(move |env, data| {
            match data {
                ScVal::Map(Some(entries)) => {
                    for entry in entries.iter() {
                        let key_matched = match &entry.key {
                            ScVal::Symbol(sym) => {
                                AsRef::<[u8]>::as_ref(sym) == field_name.as_bytes()
                            }
                            _ => false,
                        };
                        if key_matched {
                            match read_as::<T>(env, &entry.val) {
                                Some(val) if val == expected => return Ok(()),
                                Some(val) => {
                                    return Err(format!(
                                        "field {:?} has value {:?}, expected {:?}",
                                        field_name, val, expected
                                    ))
                                }
                                None => {
                                    return Err(format!(
                                        "field {:?} could not be converted to expected type",
                                        field_name
                                    ))
                                }
                            }
                        }
                    }
                    Err(format!(
                        "field {:?} not found in event data map",
                        field_name
                    ))
                }
                other => {
                    // If data is a direct value and field_name is "" or "value"
                    if field_name.is_empty() || field_name == "value" {
                        match read_as::<T>(env, other) {
                            Some(val) if val == expected => Ok(()),
                            _ => Err(format!("expected direct data value {:?}", expected)),
                        }
                    } else {
                        Err(format!(
                            "event data is not a map, cannot inspect field {:?}",
                            field_name
                        ))
                    }
                }
            }
        }));
        self
    }

    pub fn with_data_predicate<F>(&mut self, pred: F) -> &mut Self
    where
        F: Fn(&Env, &ScVal) -> bool + 'a,
    {
        self.data_predicates.push(Box::new(move |env, data| {
            if pred(env, data) {
                Ok(())
            } else {
                Err("data predicate returned false".to_string())
            }
        }));
        self
    }

    /// Returns all events in scope.
    fn scoped_events(&self) -> Vec<ContractEvent> {
        match self.events {
            Some(collected) => collected.to_vec(),
            None => {
                use soroban_sdk::testutils::Events as _;
                self.env.events().all().events().to_vec()
            }
        }
    }

    /// Evaluates whether an event satisfies the matcher's constraints.
    pub fn matches_event(&self, event: &ContractEvent) -> Result<(), String> {
        // 1. Contract filter
        if let Some(wanted_contract) = &self.contract {
            let wanted_emitter = crate::events::emitting_contract(wanted_contract);
            match (wanted_emitter.as_ref(), event.contract_id.as_ref()) {
                (Some(w), Some(e)) if w == e => {}
                _ => return Err("contract id did not match".to_string()),
            }
        }

        // 2. Topic pattern filter
        if let Some(pattern) = &self.topic_pattern {
            if !event_matches_topic_pattern(event, pattern) {
                return Err(format!(
                    "topics {:?} did not match pattern {:?}",
                    event_topics(event),
                    pattern
                ));
            }
        }

        // 3. Data predicates
        let raw_data = event_data(event);
        for pred in &self.data_predicates {
            pred(self.env, &raw_data)?;
        }

        Ok(())
    }

    /// Asserts that at least one emitted event matches all configured criteria.
    ///
    /// If no match is found, panics with a detailed summary of expected criteria
    /// and all observed events.
    pub fn assert_emitted(&self) {
        let all_events = self.scoped_events();
        for event in &all_events {
            if self.matches_event(event).is_ok() {
                return;
            }
        }

        // Format detailed failure diagnostic
        let mut expected_desc = Vec::new();
        if let Some(c) = &self.contract {
            expected_desc.push(format!("contract: {:?}", c));
        }
        if let Some(tp) = &self.topic_pattern {
            expected_desc.push(format!("topic pattern: {:?}", tp));
        }
        if !self.data_predicates.is_empty() {
            expected_desc.push(format!("{} data predicate(s)", self.data_predicates.len()));
        }
        let expected_str = if expected_desc.is_empty() {
            "(any event)".to_string()
        } else {
            expected_desc.join(", ")
        };

        let mut observed_desc = Vec::new();
        for (i, event) in all_events.iter().enumerate() {
            let contract_str = event
                .contract_id
                .as_ref()
                .map(|id| format!("{:?}", id))
                .unwrap_or_else(|| "(none)".to_string());
            let topics_str = event_topics(event).join(", ");
            let data_str = format!("{:?}", event_data(event));
            observed_desc.push(format!(
                "  [{}] contract: {}, topics: [{}], data: {}",
                i + 1,
                contract_str,
                topics_str,
                data_str
            ));
        }

        let observed_str = if observed_desc.is_empty() {
            "  (no events emitted)".to_string()
        } else {
            observed_desc.join("\n")
        };

        panic!(
            "assert_emitted_matches! failed: no event matched expected criteria [{}].\n\
             Observed {} event(s):\n{}",
            expected_str,
            all_events.len(),
            observed_str
        );
    }
}

/// Asserts that an event was emitted matching glob wildcards or regular expression topic patterns.
///
/// Supports optional contract address filtering, topic wildcard/regex matching,
/// and data field verification.
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_emitted_matches;
///
/// assert_emitted_matches!(env, {
///     contract: dex_id,
///     topic: "Trade:*",
///     data: { amount: 500u64 },
/// });
///
/// assert_emitted_matches!(env, {
///     topic: r"^Order:(Filled|Cancelled)$",
/// });
/// ```
#[macro_export]
macro_rules! assert_emitted_matches {
    ($source:expr, { $($fields:tt)* }) => {{
        let mut matcher = $crate::regex_matcher::RegexEventMatcher::new(&$source);
        $crate::__assert_emitted_matches_fields!(matcher; $($fields)*);
        matcher.assert_emitted();
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __assert_emitted_matches_fields {
    ($matcher:expr;) => {};
    ($matcher:expr; contract: $contract:expr $(, $($rest:tt)*)?) => {
        $matcher.from_contract(&$contract);
        $crate::__assert_emitted_matches_fields!($matcher; $($($rest)*)?);
    };
    ($matcher:expr; topic: $topic:expr $(, $($rest:tt)*)?) => {
        $matcher.with_topic_pattern($topic);
        $crate::__assert_emitted_matches_fields!($matcher; $($($rest)*)?);
    };
    ($matcher:expr; data: { $($key:ident : $val:expr),* $(,)? } $(, $($rest:tt)*)?) => {
        $(
            $matcher.with_data_field(stringify!($key), $val);
        )*
        $crate::__assert_emitted_matches_fields!($matcher; $($($rest)*)?);
    };
    ($matcher:expr; data: $data_pred:expr $(, $($rest:tt)*)?) => {
        $matcher.with_data_predicate($data_pred);
        $crate::__assert_emitted_matches_fields!($matcher; $($($rest)*)?);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventLog;
    use soroban_sdk::{
        contract, contractimpl, contracttype, symbol_short, Env, String as SorobanString, Symbol,
    };

    #[test]
    fn test_glob_matching() {
        assert!(glob_matches("Transfer:*", "Transfer:XLM"));
        assert!(glob_matches("Transfer:*", "Transfer:USDC"));
        assert!(!glob_matches("Transfer:*", "Mint:XLM"));

        assert!(glob_matches("Order:*:104", "Order:Created:104"));
        assert!(glob_matches("Order:*:104", "Order:Cancelled:104"));
        assert!(!glob_matches("Order:*:104", "Order:Created:105"));

        assert!(glob_matches("Trade:?", "Trade:1"));
        assert!(!glob_matches("Trade:?", "Trade:12"));
    }

    #[test]
    fn test_regex_matching() {
        assert!(regex_matches(r"^Order:(Filled|Cancelled)$", "Order:Filled"));
        assert!(regex_matches(
            r"^Order:(Filled|Cancelled)$",
            "Order:Cancelled"
        ));
        assert!(!regex_matches(
            r"^Order:(Filled|Cancelled)$",
            "Order:Pending"
        ));

        assert!(regex_matches(r"^Trade:\d+$", "Trade:500"));
        assert!(!regex_matches(r"^Trade:\d+$", "Trade:abc"));
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct TradePayload {
        pub amount: u64,
        pub price: i128,
    }

    #[contract]
    pub struct TestEventEmitter;

    #[contractimpl]
    impl TestEventEmitter {
        #[allow(deprecated)]
        pub fn emit_trade(env: Env, symbol: Symbol, amount: u64, price: i128) {
            env.events().publish(
                (symbol_short!("Trade"), symbol),
                TradePayload { amount, price },
            );
        }

        #[allow(deprecated)]
        pub fn emit_order(env: Env, status: SorobanString) {
            env.events().publish((symbol_short!("Order"), status), ());
        }
    }

    #[test]
    fn test_assert_emitted_matches_glob_topic() {
        let env = Env::default();
        let contract_id = env.register(TestEventEmitter, ());
        let client = TestEventEmitterClient::new(&env, &contract_id);

        client.emit_trade(&symbol_short!("XLM"), &500u64, &100i128);

        assert_emitted_matches!(env, {
            contract: contract_id,
            topic: "Trade:*",
        });

        assert_emitted_matches!(env, {
            topic: "*:XLM",
        });
    }

    #[test]
    fn test_assert_emitted_matches_regex_topic() {
        let env = Env::default();
        let contract_id = env.register(TestEventEmitter, ());
        let client = TestEventEmitterClient::new(&env, &contract_id);

        let status = SorobanString::from_str(&env, "Filled");
        client.emit_order(&status);

        assert_emitted_matches!(env, {
            contract: contract_id,
            topic: r"^Order:(Filled|Cancelled)$",
        });
    }

    #[test]
    fn test_assert_emitted_matches_with_data_fields() {
        let env = Env::default();
        let contract_id = env.register(TestEventEmitter, ());
        let client = TestEventEmitterClient::new(&env, &contract_id);

        client.emit_trade(&symbol_short!("USDC"), &1000u64, &250i128);

        assert_emitted_matches!(env, {
            contract: contract_id,
            topic: "Trade:USDC",
            data: {
                amount: 1000u64,
                price: 250i128,
            },
        });
    }

    #[test]
    fn test_assert_emitted_matches_with_event_log() {
        let env = Env::default();
        let contract_id = env.register(TestEventEmitter, ());
        let client = TestEventEmitterClient::new(&env, &contract_id);

        let mut log = EventLog::new(&env);

        client.emit_trade(&symbol_short!("XLM"), &100u64, &50i128);
        log.collect();

        let status = SorobanString::from_str(&env, "Cancelled");
        client.emit_order(&status);
        log.collect();

        // Single invocation only sees latest (Order:Cancelled)
        assert_emitted_matches!(env, {
            topic: "Order:*",
        });

        // EventLog sees both historical invocations
        assert_emitted_matches!(log, {
            contract: contract_id,
            topic: "Trade:*",
        });
        assert_emitted_matches!(log, {
            contract: contract_id,
            topic: "Order:*",
        });
    }

    #[test]
    #[should_panic(expected = "assert_emitted_matches! failed: no event matched expected criteria")]
    fn test_assert_emitted_matches_negative_case_fails() {
        let env = Env::default();
        let contract_id = env.register(TestEventEmitter, ());
        let client = TestEventEmitterClient::new(&env, &contract_id);

        client.emit_trade(&symbol_short!("XLM"), &500u64, &100i128);

        assert_emitted_matches!(env, {
            topic: "NonExistentTopic:*",
        });
    }
}
