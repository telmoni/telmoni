//! Typed evaluators, on a model speaking System One — TypeSafe's Jev, or an
//! open-weight one served in its shape: a state, the target alone and never
//! an instruction, and questions, each answered as a score of its own with
//! its probabilities. It writes no reasoning and cannot abstain.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use telmoni_shared::model::systemone::{Answer, Criteria, Decided, Question};
use telmoni_shared::{ScoreType, TelmoniError};

use super::{MEANS_MAX, MESSAGE_MAX, VARIABLES, fill, named};
use crate::handler::model_connections::MODEL_MAX;
use crate::scores::{CATEGORY_MAX, Judgement, NAME_MAX as SCORE_NAME_MAX};

/// The most questions a typed evaluator asks, levels a score's question
/// has, and options a choice's.
pub const QUESTIONS_MAX: usize = 16;
pub const LEVELS_MAX: usize = 10;
pub const OPTIONS_MAX: usize = 255;
/// The longest a question's instructions run.
pub const INSTRUCTIONS_MAX: usize = 4_000;
/// Jev's context for the state and its longest question, in tokens, and the
/// characters a token is counted as, low so English never runs past it.
pub const CONTEXT_TOKENS: usize = 32_000;
const CHARS_PER_TOKEN: usize = 3;

/// What a typed evaluator asks: its state's template and its questions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Typed {
    pub state: String,
    pub questions: Vec<Question>,
}

/// Whether `text` is described in 1 to [`MEANS_MAX`] characters.
fn described(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= MEANS_MAX
}

/// `question` held to its kind's rules, each text trimmed.
fn question_checked(mut question: Question) -> Result<Question, String> {
    question.name = question.name.trim().to_owned();
    if question.name.is_empty() || question.name.chars().count() > SCORE_NAME_MAX {
        return Err(format!(
            "a question's name, its score's, is 1 to {SCORE_NAME_MAX} characters"
        ));
    }
    question.instructions = question.instructions.trim().to_owned();
    if question.instructions.is_empty() || question.instructions.chars().count() > INSTRUCTIONS_MAX
    {
        return Err(format!(
            "a question's instructions run to 1 to {INSTRUCTIONS_MAX} characters"
        ));
    }
    match &mut question.criteria {
        Criteria::Noul {
            yes_means,
            no_means,
        } => {
            *yes_means = yes_means.trim().to_owned();
            *no_means = no_means.trim().to_owned();
            let both = described(yes_means) && described(no_means);
            if !both && !(yes_means.is_empty() && no_means.is_empty()) {
                return Err(format!(
                    "a yes-or-no question describes both answers or neither, each in up to {MEANS_MAX} characters"
                ));
            }
        }
        Criteria::Score { levels } => {
            for level in levels.iter_mut() {
                *level = level.trim().to_owned();
            }
            if !(2..=LEVELS_MAX).contains(&levels.len()) || !levels.iter().all(|l| described(l)) {
                return Err(format!(
                    "a score's question has 2 to {LEVELS_MAX} levels, the lowest first, each described in up to {MEANS_MAX} characters"
                ));
            }
        }
        Criteria::Choice { options } => {
            for option in options.iter_mut() {
                option.name = option.name.trim().to_owned();
                option.means = option.means.trim().to_owned();
            }
            let distinct: BTreeSet<&str> = options.iter().map(|o| o.name.as_str()).collect();
            if !(2..=OPTIONS_MAX).contains(&options.len())
                || distinct.len() != options.len()
                || options.iter().any(|o| {
                    o.name.is_empty()
                        || o.name.chars().count() > CATEGORY_MAX
                        || o.means.chars().count() > MEANS_MAX
                })
            {
                return Err(format!(
                    "a choice's question has 2 to {OPTIONS_MAX} distinct options, each named in 1 to {CATEGORY_MAX} characters and described in up to {MEANS_MAX}"
                ));
            }
        }
    }
    Ok(question)
}

/// The characters a question adds to the state's, as the context counts it.
fn length_of(question: &Question) -> usize {
    let chars = |text: &str| text.chars().count();
    let criteria = match &question.criteria {
        Criteria::Noul {
            yes_means,
            no_means,
        } => chars(yes_means) + chars(no_means),
        Criteria::Score { levels } => levels.iter().map(|l| chars(l)).sum(),
        Criteria::Choice { options } => options
            .iter()
            .map(|o| chars(&o.name) + chars(&o.means))
            .sum(),
    };
    chars(&question.name) + chars(&question.instructions) + criteria
}

/// The most of the model's context, in characters, a question may take, so
/// half is always the state's.
pub const QUESTION_CHARS_MAX: usize = CONTEXT_TOKENS * CHARS_PER_TOKEN / 2;

/// `probabilities` of the levels or options a question names alone, so an
/// endpoint's other keys never ride on a score.
fn only(
    probabilities: &BTreeMap<String, f64>,
    named: impl Iterator<Item = String>,
) -> BTreeMap<String, f64> {
    named
        .filter_map(|name| probabilities.get(&name).map(|p| (name, *p)))
        .collect()
}

impl Typed {
    /// The evaluator held to a typed one's rules: a state of 1 to
    /// [`MESSAGE_MAX`] characters naming known variables, never its
    /// corrections, and 1 to [`QUESTIONS_MAX`] questions of distinct names.
    ///
    /// # Errors
    ///
    /// A 400 naming the rule broken.
    pub fn checked(mut self) -> Result<Self, TelmoniError> {
        let refuse = |why: String| Err(TelmoniError::BadRequest(why));
        self.state = self.state.trim().to_owned();
        if self.state.is_empty() || self.state.chars().count() > MESSAGE_MAX {
            return refuse(format!(
                "a typed evaluator's state runs to 1 to {MESSAGE_MAX} characters"
            ));
        }
        if let Some(unknown) = self
            .variables()
            .into_iter()
            .find(|v| v == "examples" || !VARIABLES.contains(&v.as_str()))
        {
            return refuse(format!(
                "{{{{{unknown}}}}} is no variable a state names: it holds what is judged alone"
            ));
        }
        if self.questions.is_empty() || self.questions.len() > QUESTIONS_MAX {
            return refuse(format!(
                "a typed evaluator asks 1 to {QUESTIONS_MAX} questions"
            ));
        }
        let mut questions = Vec::with_capacity(self.questions.len());
        for question in self.questions {
            let question = question_checked(question).map_err(TelmoniError::BadRequest)?;
            if length_of(&question) > QUESTION_CHARS_MAX {
                return refuse(format!(
                    "a question, its instructions and what each answer means, runs to {QUESTION_CHARS_MAX} characters at most, leaving the state room"
                ));
            }
            questions.push(question);
        }
        let names: BTreeSet<&str> = questions.iter().map(|q| q.name.as_str()).collect();
        if names.len() != questions.len() {
            return refuse("each question is named once, as its score is".into());
        }
        self.questions = questions;
        Ok(self)
    }

    /// Each variable the state names, once.
    pub fn variables(&self) -> BTreeSet<String> {
        named(&self.state).into_iter().map(str::to_owned).collect()
    }

    /// The state as sent, each variable `filled` holds replaced, and whether
    /// it was cut to leave its longest question room in the model's context.
    pub fn rendered(&self, filled: &BTreeMap<&str, String>) -> (String, bool) {
        let state = fill(&self.state, filled);
        let longest = self.questions.iter().map(length_of).max().unwrap_or(0);
        let room = (CONTEXT_TOKENS * CHARS_PER_TOKEN).saturating_sub(longest);
        if state.chars().count() <= room {
            return (state, false);
        }
        let marker = "\n[cut: the rest was not sent]";
        let mut cut: String = state
            .chars()
            .take(room.saturating_sub(marker.len()))
            .collect();
        cut.push_str(marker);
        (cut, true)
    }

    /// Each question's answer read as its score, with the answer as kept on
    /// it: its probabilities, its confidence and the version that answered.
    ///
    /// # Errors
    ///
    /// A question left unanswered, or an answer outside its question, named.
    pub fn judgements(&self, decided: &Decided) -> Result<Vec<(Judgement, Value)>, String> {
        self.questions
            .iter()
            .map(|question| {
                let outside = |why: &str| {
                    format!(
                        "the model answered \"{}\" outside its question: {why}",
                        question.name
                    )
                };
                let given = decided
                    .answers
                    .get(&question.name)
                    .ok_or_else(|| outside("no answer"))?;
                let (data_type, value, categories, range, answer) =
                    match (&question.criteria, given) {
                        (Criteria::Noul { .. }, Answer::Noul { noul }) => (
                            ScoreType::Boolean,
                            Some(if *noul > 0.5 { 1.0 } else { 0.0 }),
                            Vec::new(),
                            (None, None),
                            json!({ "noul": noul }),
                        ),
                        (
                            Criteria::Score { levels },
                            Answer::Score {
                                score,
                                confidence,
                                probabilities,
                            },
                        ) => {
                            // A value past its levels is refused by the score's own check.
                            let top = f64::from(
                                u8::try_from(levels.len().saturating_sub(1)).unwrap_or(u8::MAX),
                            );
                            (
                                ScoreType::Number,
                                Some(*score),
                                Vec::new(),
                                (Some(0.0), Some(top)),
                                json!({
                                    "score": score,
                                    "confidence": confidence,
                                    "probabilities": only(probabilities, (0..levels.len()).map(|n| n.to_string())),
                                }),
                            )
                        }
                        (
                            Criteria::Choice { options },
                            Answer::Choice {
                                choice,
                                confidence,
                                probabilities,
                            },
                        ) => {
                            if !options.iter().any(|o| &o.name == choice) {
                                return Err(outside("an option it does not name"));
                            }
                            (
                                ScoreType::Category,
                                None,
                                vec![choice.clone()],
                                (None, None),
                                json!({
                                    "confidence": confidence,
                                    "probabilities": only(probabilities, options.iter().map(|o| o.name.clone())),
                                }),
                            )
                        }
                        _ => return Err(outside("an answer of another type")),
                    };
                let mut answer = answer;
                if let Some(kept) = answer.as_object_mut() {
                    let model: String = decided.model.chars().take(MODEL_MAX).collect();
                    kept.insert("model".into(), json!(model));
                }
                let judgement = Judgement {
                    name: question.name.clone(),
                    data_type,
                    value,
                    categories,
                    range_min: range.0,
                    range_max: range.1,
                }
                .checked()
                .map_err(|e| outside(&e.to_string()))?;
                Ok((judgement, answer))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telmoni_shared::model::Usage;
    use telmoni_shared::model::systemone::Choice;

    fn typed() -> Typed {
        Typed {
            state: "Asked: {{input}}\nAnswered: {{ output }}".into(),
            questions: vec![
                Question {
                    name: " grounded ".into(),
                    instructions: "Does the answer rest on what was asked?".into(),
                    criteria: Criteria::Noul {
                        yes_means: String::new(),
                        no_means: String::new(),
                    },
                },
                Question {
                    name: "complete".into(),
                    instructions: "How far was the task done?".into(),
                    criteria: Criteria::Score {
                        levels: vec!["no".into(), "partly".into(), "fully".into()],
                    },
                },
                Question {
                    name: "topic".into(),
                    instructions: "What is it about?".into(),
                    criteria: Criteria::Choice {
                        options: vec![
                            Choice {
                                name: "billing".into(),
                                means: String::new(),
                            },
                            Choice {
                                name: "other".into(),
                                means: String::new(),
                            },
                        ],
                    },
                },
            ],
        }
    }

    /// A state names what is judged alone, never its corrections; questions
    /// are named once, a score's levels and a choice's options two at least.
    #[test]
    fn a_typed_evaluator_is_held_to_its_rules() {
        let kept = typed().checked().expect("kept");
        assert_eq!(kept.questions[0].name, "grounded");
        let mut examples = typed();
        examples.state = "{{examples}} {{input}}".into();
        assert!(examples.checked().is_err());
        let mut twice = typed();
        twice.questions[1].name = "grounded".into();
        assert!(twice.checked().is_err());
        let mut one_level = typed();
        one_level.questions[1].criteria = Criteria::Score {
            levels: vec!["only".into()],
        };
        assert!(one_level.checked().is_err());
        let mut half = typed();
        half.questions[0].criteria = Criteria::Noul {
            yes_means: "yes".into(),
            no_means: String::new(),
        };
        assert!(half.checked().is_err(), "both answers or neither");
        let mut none = typed();
        none.questions.clear();
        assert!(none.checked().is_err());
        let mut silent = typed();
        silent.questions[2].instructions = " ".into();
        assert!(silent.checked().is_err());
        let mut long = typed();
        long.questions[2].criteria = Criteria::Choice {
            options: (0..255)
                .map(|n| Choice {
                    name: format!("option {n}"),
                    means: "m".repeat(500),
                })
                .collect(),
        };
        assert!(long.checked().is_err(), "a question leaves the state room");
    }

    /// A state past the model's context, its longest question beside it, is
    /// cut and says so.
    #[test]
    fn a_state_past_the_context_is_cut() {
        let kept = typed().checked().expect("kept");
        let short = BTreeMap::from([("input", "2+2?".to_owned()), ("output", "4".to_owned())]);
        assert_eq!(
            kept.rendered(&short),
            ("Asked: 2+2?\nAnswered: 4".to_owned(), false)
        );
        let long = BTreeMap::from([("input", "x".repeat(200_000)), ("output", String::new())]);
        let (state, cut) = kept.rendered(&long);
        assert!(cut);
        assert!(state.ends_with("[cut: the rest was not sent]"));
        assert!(state.chars().count() <= CONTEXT_TOKENS * CHARS_PER_TOKEN);
    }

    /// Each answer is its question's score — a yes past one half, a score's
    /// weighted value on its levels, a choice's option — with its answer kept;
    /// one missing or outside its question judges nothing.
    #[test]
    fn answers_are_read_as_scores() {
        let kept = typed().checked().expect("kept");
        let decided = |answers: Vec<(&str, Answer)>| Decided {
            model: "jev-1.14.0".into(),
            answers: answers
                .into_iter()
                .map(|(n, a)| (n.to_owned(), a))
                .collect(),
            usage: Usage::default(),
        };
        let all = || {
            vec![
                ("grounded", Answer::Noul { noul: 0.51 }),
                (
                    "complete",
                    Answer::Score {
                        score: 1.4,
                        confidence: Some(0.6),
                        probabilities: BTreeMap::from([("0".into(), 0.1), ("note".into(), 0.9)]),
                    },
                ),
                (
                    "topic",
                    Answer::Choice {
                        choice: "billing".into(),
                        confidence: Some(0.9),
                        probabilities: BTreeMap::new(),
                    },
                ),
            ]
        };
        let judged = kept.judgements(&decided(all())).expect("judged");
        assert_eq!(judged[0].0.value, Some(1.0));
        assert_eq!(judged[0].1["noul"], 0.51);
        assert_eq!(judged[0].1["model"], "jev-1.14.0");
        assert_eq!(
            (judged[1].0.value, judged[1].0.range_max),
            (Some(1.4), Some(2.0))
        );
        assert_eq!(
            judged[1].1["probabilities"],
            json!({ "0": 0.1 }),
            "only the levels it names"
        );
        assert_eq!(judged[2].0.categories, ["billing"]);
        assert_eq!(judged[2].0.value, None);

        let mut low = all();
        low[0] = ("grounded", Answer::Noul { noul: 0.5 });
        assert_eq!(
            kept.judgements(&decided(low)).expect("judged")[0].0.value,
            Some(0.0)
        );
        let mut past = all();
        past[1] = (
            "complete",
            Answer::Score {
                score: 2.5,
                confidence: None,
                probabilities: BTreeMap::new(),
            },
        );
        assert!(kept.judgements(&decided(past)).is_err());
        let mut unnamed = all();
        unnamed[2] = (
            "topic",
            Answer::Choice {
                choice: "weather".into(),
                confidence: None,
                probabilities: BTreeMap::new(),
            },
        );
        assert!(kept.judgements(&decided(unnamed)).is_err());
        let mut swapped = all();
        swapped[0] = ("grounded", Answer::Noul { noul: 0.9 });
        swapped[1] = ("complete", Answer::Noul { noul: 0.9 });
        assert!(kept.judgements(&decided(swapped)).is_err());
        assert!(kept.judgements(&decided(all().split_off(1))).is_err());
    }
}
