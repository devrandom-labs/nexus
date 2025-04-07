#![allow(dead_code)]
use super::error::Error;
use std::fmt::{Display, Formatter};

static MIN_PASSWORD_LENGTH: u8 = 8;
static REQUIRED_SYMBOL: &str = "!@#$%^&*()_+-=[]{};':\"\\|,.<>/?~";

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum PasswordRule {
    TooShort(u8),
    MissingLowercase,
    MissingUppercase,
    MissingDigit,
    MissingSymbol,
}

impl Display for PasswordRule {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match *self {
            PasswordRule::TooShort(min) => write!(f, "must be at least {min} characters"),
            PasswordRule::MissingLowercase => write!(f, "must include a lower case letter"),
            PasswordRule::MissingUppercase => write!(f, "must include an upper case letter"),
            PasswordRule::MissingDigit => write!(f, "must include a digit"),
            PasswordRule::MissingSymbol => {
                let example = &REQUIRED_SYMBOL[..std::cmp::min(5, REQUIRED_SYMBOL.len())];
                write!(f, "must include a symbol (e.g., \"{}...\")", example)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PasswordValidationErrors(pub Vec<PasswordRule>);

impl Display for PasswordValidationErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let message: Vec<String> = self.0.iter().map(|r| r.to_string()).collect();
        write!(f, "Password validation failed: {}", message.join(", "))
    }
}

impl PasswordValidationErrors {
    pub fn new() -> Self {
        Self(vec![])
    }

    pub fn add(&mut self, rule: PasswordRule) {
        self.0.push(rule);
    }
}

#[inline]
pub fn validate(password: &str) -> Result<(), Error> {
    if password.len() < MIN_PASSWORD_LENGTH.into() {
        return Err(Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::TooShort(MIN_PASSWORD_LENGTH),
        ])));
    }
    let mut found_lower_case = false;
    let mut found_upper_case = false;
    let mut found_digit = false;
    let mut found_symbol = false;

    for c in password.chars() {
        if !found_lower_case && c.is_lowercase() {
            found_lower_case = true;
        } else if !found_upper_case && c.is_uppercase() {
            found_upper_case = true;
        } else if !found_digit && c.is_numeric() {
            found_digit = true;
        } else if !found_symbol && REQUIRED_SYMBOL.contains(c) {
            found_symbol = true;
        }

        if found_digit && found_lower_case && found_upper_case && found_symbol {
            break;
        }
    }

    if found_lower_case && found_upper_case && found_digit && found_symbol {
        Ok(())
    } else {
        let mut errors = PasswordValidationErrors::new();
        if !found_lower_case {
            errors.add(PasswordRule::MissingLowercase);
        };

        if !found_upper_case {
            errors.add(PasswordRule::MissingUppercase);
        };

        if !found_digit {
            errors.add(PasswordRule::MissingDigit);
        };

        if !found_symbol {
            errors.add(PasswordRule::MissingSymbol);
        };

        Err(Error::PasswordValidation(errors))
    }
}

#[cfg(test)]
mod test {
    use crate::services::password_validator::PasswordRule;

    use super::{Error, PasswordValidationErrors, validate};

    #[test]
    fn test_valid_password() {
        assert_eq!(validate("ValidP@ss1"), Ok(()));
        assert_eq!(validate("AnotherV@lid_123"), Ok(()));
        // Test with exact minimum length
        assert_eq!(validate("Val!dPa8"), Ok(()));
    }

    #[test]
    fn test_short_password() {
        let expected_err =
            Error::PasswordValidation(PasswordValidationErrors(vec![PasswordRule::TooShort(8)]));
        assert_eq!(validate("Short1!"), Err(expected_err.clone()));
        assert_eq!(validate("V@lid7"), Err(expected_err.clone()));
        // Test empty string
        assert_eq!(validate(""), Err(expected_err));
    }

    #[test]
    fn test_no_lower_case_password() {
        let expected_err = Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::MissingLowercase,
        ]));
        assert_eq!(validate("NOLOWER!1"), Err(expected_err.clone()));
        assert_eq!(validate("ALLUPPER&7"), Err(expected_err));
    }

    #[test]
    fn test_no_upper_case_password() {
        let expected_err = Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::MissingUppercase,
        ]));
        assert_eq!(validate("noupper!1"), Err(expected_err.clone()));
        assert_eq!(validate("alllower&7"), Err(expected_err));
    }

    #[test]
    fn test_no_digit_password() {
        let expected_err =
            Error::PasswordValidation(PasswordValidationErrors(vec![PasswordRule::MissingDigit]));
        assert_eq!(validate("NoDigit!Pwd"), Err(expected_err.clone()));
        assert_eq!(validate("NoDigit@Too"), Err(expected_err));
    }

    #[test]
    fn test_no_symbol_password() {
        let expected_err =
            Error::PasswordValidation(PasswordValidationErrors(vec![PasswordRule::MissingSymbol]));
        assert_eq!(validate("NoSymbol123"), Err(expected_err.clone()));
        assert_eq!(
            validate("NoSymbolToo"),
            Err(Error::PasswordValidation(PasswordValidationErrors(vec![
                PasswordRule::MissingDigit,
                PasswordRule::MissingSymbol,
            ])))
        );
    }

    #[test]
    fn test_multiple_missing_requirements() {
        // Missing uppercase, digit, symbol
        let expected_err_1 = Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::MissingUppercase,
            PasswordRule::MissingDigit,
            PasswordRule::MissingSymbol,
        ]));
        assert_eq!(validate("onlylower"), Err(expected_err_1));

        // Missing lowercase, symbol
        let expected_err_2 = Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::MissingLowercase,
            PasswordRule::MissingSymbol,
        ]));
        assert_eq!(validate("ONLYUPPER123"), Err(expected_err_2));

        // Missing only digit and symbol
        let expected_err_3 = Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::MissingDigit,
            PasswordRule::MissingSymbol,
        ]));
        assert_eq!(validate("LowerAndUpper"), Err(expected_err_3));
    }

    #[test]
    fn test_min_length_but_invalid() {
        let expected_err = Error::PasswordValidation(PasswordValidationErrors(vec![
            PasswordRule::MissingUppercase,
            PasswordRule::MissingDigit,
        ]));
        assert_eq!(validate("lower!aa"), Err(expected_err));
    }

    #[test]
    fn test_valid_symbols_are_checked() {
        // Ensure various allowed symbols work
        assert!(validate("Val!dP@ss1").is_ok());
        assert!(validate("Val#dP$ss1").is_ok());
        assert!(validate("Val%dP^ss1").is_ok());
        assert!(validate("Val&dP*ss1").is_ok());
        assert!(validate("Val(dP)ss1").is_ok());
        assert!(validate("Val-dP_ss1").is_ok());
        assert!(validate("Val=dP+ss1").is_ok());
        assert!(validate("Val[dP]ss1").is_ok());
        assert!(validate("Val{dP}ss1").is_ok());
        assert!(validate("Val;dP:ss1").is_ok());
        // Note: single quote might cause issues depending on string literal handling if not careful
        assert!(validate("Val'dP\"ss1").is_ok());
        assert!(validate("Val\\dP|ss1").is_ok()); // Escaped backslash
        assert!(validate("Val,dP.ss1").is_ok());
        assert!(validate("Val<dP>ss1").is_ok());
        assert!(validate("Val/dP?ss1").is_ok());
        assert!(validate("Val`dP~ss1").is_ok()); // Backtick isn't in the list, this should fail! Let's fix the assertion
        // assert!(validate("Val`dP~ss1").is_ok()); // Fails as ` is not in REQUIRED_SYMBOLS
        assert_eq!(validate("Val`dP~ss1"), Ok(()));
        let expected_err =
            Error::PasswordValidation(PasswordValidationErrors(vec![PasswordRule::MissingSymbol]));
        assert_eq!(validate("NoValidSymb`1"), Err(expected_err));
    }
}
