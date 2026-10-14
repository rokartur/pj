//! Port of rpiv-ask-user-question: the model asks 1-4 multiple-choice questions and
//! the user answers each with option numbers or their own text.
//!
//! shortcut: questions show as numbered text answered from the composer, not as an
//! arrow-key picker with previews; build a picker widget if typing numbers gets old.

use anyhow::{Result, bail};
use serde::Deserialize;

/// Prefix of the stdin request ids this tool sends, so the client can tell its
/// questions from a shell command waiting for input.
pub const REQUEST_PREFIX: &str = "ask_user_question:";

#[derive(Debug, Deserialize)]
pub struct Questions {
    pub questions: Vec<Question>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<Choice>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub label: String,
    pub description: String,
}

impl Questions {
    pub fn validate(&self) -> Result<()> {
        if !(1..=4).contains(&self.questions.len()) {
            bail!("ask 1 to 4 questions");
        }
        for question in &self.questions {
            if !(2..=4).contains(&question.options.len()) {
                bail!("question {:?} needs 2 to 4 options", question.header);
            }
        }
        Ok(())
    }
}

impl Question {
    /// The text shown to the user.
    pub fn prompt(&self) -> String {
        let mut prompt = format!("[{}] {}\n", self.header, self.question);
        for (index, choice) in self.options.iter().enumerate() {
            prompt.push_str(&format!(
                "  {}. {}: {}\n",
                index + 1,
                choice.label,
                choice.description
            ));
        }
        prompt.push_str(if self.multi_select {
            "Answer with numbers (e.g. 1,3) or your own text."
        } else {
            "Answer with a number or your own text."
        });
        prompt
    }

    /// Option numbers become their labels; anything else is the user's own answer.
    pub fn answer(&self, input: &str) -> String {
        let input = input.trim();
        let picked: Option<Vec<&str>> = input
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|token| !token.is_empty())
            .map(|token| {
                let number = token.parse::<usize>().ok()?;
                let choice = self.options.get(number.checked_sub(1)?)?;
                Some(choice.label.as_str())
            })
            .collect();
        match picked {
            Some(labels) if labels.len() == 1 || (self.multi_select && !labels.is_empty()) => {
                labels.join(", ")
            }
            _ => input.to_string(),
        }
    }
}

/// What the model reads back, in the shape pi's tool used.
pub fn summary(answers: &[(&str, String)]) -> String {
    let pairs: Vec<String> = answers
        .iter()
        .map(|(question, answer)| format!("{question:?}={answer:?}"))
        .collect();
    format!(
        "User has answered your questions: {}. You can now continue with the user's answers in mind.",
        pairs.join(". ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(multi_select: bool) -> Question {
        Question {
            question: "Which?".into(),
            header: "Pick".into(),
            options: ["A", "B", "C"]
                .map(|label| Choice {
                    label: label.into(),
                    description: format!("about {label}"),
                })
                .into(),
            multi_select,
        }
    }

    #[test]
    fn numbers_pick_labels_and_other_text_is_kept() {
        let single = question(false);
        assert_eq!(single.answer(" 2 "), "B");
        assert_eq!(single.answer("1,3"), "1,3");
        assert_eq!(single.answer("4"), "4");
        assert_eq!(single.answer("neither, do X"), "neither, do X");
        assert_eq!(question(true).answer("1, 3"), "A, C");
    }

    #[test]
    fn prompt_numbers_the_options_and_summary_quotes_answers() {
        assert_eq!(
            question(false).prompt(),
            "[Pick] Which?\n  1. A: about A\n  2. B: about B\n  3. C: about C\nAnswer with a number or your own text."
        );
        assert_eq!(
            summary(&[("Which?", "B".into())]),
            "User has answered your questions: \"Which?\"=\"B\". You can now continue with the user's answers in mind."
        );
    }

    #[test]
    fn rejects_no_questions_or_too_few_options() {
        let one_option = Questions {
            questions: vec![Question {
                options: vec![Choice {
                    label: "only".into(),
                    description: String::new(),
                }],
                ..question(false)
            }],
        };
        assert!(one_option.validate().is_err());
        assert!(
            Questions {
                questions: Vec::new()
            }
            .validate()
            .is_err()
        );
    }
}
