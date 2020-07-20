pub mod errors;
pub mod rules;
pub mod violations;
#[macro_use]
extern crate lazy_static;

use crate::errors::CheckSQLError;
use crate::rules::{
    adding_field_with_default, adding_not_nullable_field, ban_char_type, ban_drop_database,
    changing_column_type, constraint_missing_not_valid, disallow_unique_constraint,
    prefer_robust_stmts, prefer_text_field, renaming_column, renaming_table,
    require_concurrent_index_creation,
};
use crate::violations::{RuleViolation, RuleViolationKind, ViolationMessage};
use squawk_parser::ast::RootStmt;
use squawk_parser::parse::parse_sql_query;
use std::collections::HashSet;
use std::convert::TryFrom;

pub struct SquawkRule {
    pub name: RuleViolationKind,
    func: fn(&[RootStmt]) -> Vec<RuleViolation>,
    pub messages: Vec<ViolationMessage>,
}

lazy_static! {
    pub static ref RULES: Vec<SquawkRule> = vec![
        // https://www.postgresql.org/docs/10/sql-createindex.html#SQL-CREATEINDEX-CONCURRENTLY
        SquawkRule {
            name: RuleViolationKind::RequireConcurrentIndexCreation,
            func: require_concurrent_index_creation,
            messages: vec![
                ViolationMessage::Note(
                    "Creating an index blocks writes.".into()
                ),
                ViolationMessage::Help(
                    "Create the index CONCURRENTLY.".into()
                ),
            ],
        },
        // > The RENAME forms change the name of a table (or an index, sequence,
        // > view, materialized view, or foreign table), the name of an individual
        // > column in a table, or the name of a constraint of the table. There is
        // > no effect on the stored data.
        // https://www.postgresql.org/docs/10/sql-altertable.html
        SquawkRule {
            name: RuleViolationKind::RenamingColumn,
            func: renaming_column,
            messages: vec![
                ViolationMessage::Note(
                    "Renaming a column may break existing clients.".into()
                ),
            ],
        },
        // see RenamingColumn rule
        SquawkRule {
            name: RuleViolationKind::RenamingTable,
            func: renaming_table,
            messages: vec![
                ViolationMessage::Note(
                    "Renaming a table may break existing clients.".into()
                ),
            ],
        },
        // > Adding a column with a volatile DEFAULT or changing the type of an
        // > existing column will require the entire table and its indexes to be
        // > rewritten. As an exception, when changing the type of an existing
        // > column, if the USING clause does not change the column contents and
        // > the old type is either binary coercible to the new type or an
        // > unconstrained domain over the new type, a table rewrite is not
        // > needed; but any indexes on the affected columns must still be
        // > rebuilt. Table and/or index rebuilds may take a significant amount of
        // > time for a large table; and will temporarily require as much as
        // > double the disk space.
        // https://www.postgresql.org/docs/current/sql-altertable.html
        SquawkRule {
            name: RuleViolationKind::ChangingColumnType,
            func: changing_column_type,
            messages: vec![
                ViolationMessage::Note("Requires an ACCESS EXCLUSIVE lock on the table which blocks reads.".into()),
                ViolationMessage::Note("Changing the type may break existing clients.".into()),
            ],
        },
        // usually paired with a DEFAULT
        SquawkRule {
            name: RuleViolationKind::AddingNotNullableField,
            func: adding_not_nullable_field,
            messages: vec![
                // https://www.postgresql.org/docs/10/sql-altertable.html
                ViolationMessage::Note(
                    "Adding a NOT NULL field requires exclusive locks and table rewrites.".into(),
                ),
                ViolationMessage::Help("Make the field nullable.".into())
            ],
        },
        // see ChangingColumnType
        SquawkRule {
            name: RuleViolationKind::AddingFieldWithDefault,
            func: adding_field_with_default,
            messages: vec![
                ViolationMessage::Note(
                    "In Postgres versions <11 adding a field with a DEFAULT requires a table rewrite with an ACCESS EXCLUSIVE lock.".into(),
                ),
                ViolationMessage::Help(
                    "Add the field as nullable, then set a default, backfill, and remove nullabilty.".into(),
                ),

            ],
        },
        // > Although most forms of ADD table_constraint require an ACCESS
        // > EXCLUSIVE lock, ADD FOREIGN KEY requires only a SHARE ROW EXCLUSIVE
        // > lock.
        // https://www.postgresql.org/docs/current/sql-altertable.html
        SquawkRule {
            name: RuleViolationKind::DisallowedUniqueConstraint,
            func: disallow_unique_constraint,
            messages: vec![
                ViolationMessage::Note(
                    "Adding a UNIQUE constraint requires an ACCESS EXCLUSIVE lock which blocks reads.".into(),
                ),
                ViolationMessage::Help(
                    "Create an index CONCURRENTLY and create the constraint using the index.".into(),
                ),

            ],
        },
        // > Scanning a large table to verify a new foreign key or check
        // > constraint can take a long time, and other updates to the table are
        // > locked out until the ALTER TABLE ADD CONSTRAINT command is committed.
        // > The main purpose of the NOT VALID constraint option is to reduce the
        // > impact of adding a constraint on concurrent updates. With NOT VALID,
        // > the ADD CONSTRAINT command does not scan the table and can be
        // > committed immediately. After that, a VALIDATE CONSTRAINT command can
        // > be issued to verify that existing rows satisfy the constraint. The
        // > validation step does not need to lock out concurrent updates, since
        // > it knows that other transactions will be enforcing the constraint for
        // > rows that they insert or update; only pre-existing rows need to be
        // > checked. Hence, validation acquires only a SHARE UPDATE EXCLUSIVE
        // > lock on the table being altered. (If the constraint is a foreign key
        // > then a ROW SHARE lock is also required on the table referenced by the
        // > constraint.) In addition to improving concurrency, it can be useful
        // > to use NOT VALID and VALIDATE CONSTRAINT in cases where the table is
        // > known to contain pre-existing violations. Once the constraint is in
        // > place, no new violations can be inserted, and the existing problems
        // > can be corrected at leisure until VALIDATE CONSTRAINT finally
        // > succeeds.
        // https://www.postgresql.org/docs/current/sql-altertable.html#SQL-ALTERTABLE-NOTES
        SquawkRule {
            name: RuleViolationKind::ConstraintMissingNotValid,
            func: constraint_missing_not_valid,
            messages: vec![
                ViolationMessage::Note("Requires a table scan to verify constraint and an ACCESS EXCLUSIVE lock which blocks reads.".into()),
                ViolationMessage::Help("Add NOT VALID to the constraint and then VALIDATE the constraint.".into()),
            ],
        },
        SquawkRule {
            name: RuleViolationKind::BanDropDatabase,
            func: ban_drop_database,
            messages: vec![
                ViolationMessage::Note(
                    "Dropping a database may break existing clients.".into()
                )
            ],
        },
        // see ConstraintMissingNotValid for more docs
        SquawkRule {
            name: RuleViolationKind::PreferTextField,
            func: prefer_text_field,
            messages: vec![
                ViolationMessage::Note(
                    "Changing the size of a varchar field requires an ACCESS EXCLUSIVE lock.".into()
                ),
                ViolationMessage::Help(
                    "Use a text field with a check constraint.".into()
                ),
            ]
        },
        SquawkRule {
            name: RuleViolationKind::PreferRobustStmts,
            func: prefer_robust_stmts,
            messages: vec![
                ViolationMessage::Help(
                    "Consider wrapping in a transaction or adding a IF NOT EXISTS clause.".into()
                ),
            ]
        },
        SquawkRule {
            name: RuleViolationKind::BanCharField,
            func: ban_char_type,
            messages: vec![
                ViolationMessage::Help(
                    "Use text or varchar instead.".into()
                ),
            ]
        }
    ];
}

pub fn check_sql(
    sql: &str,
    excluded_rules: &[String],
) -> Result<Vec<RuleViolation>, CheckSQLError> {
    let tree = parse_sql_query(sql)?;

    let excluded_rules: HashSet<RuleViolationKind> = excluded_rules
        .iter()
        .flat_map(|s| RuleViolationKind::try_from(s.as_ref()).ok())
        .collect();

    let mut errs = vec![];
    for rule in RULES.iter().filter(|r| !excluded_rules.contains(&r.name)) {
        errs.extend((rule.func)(&tree));
    }

    errs.sort_by_key(|v| v.span.start);

    Ok(errs)
}

#[cfg(test)]
mod test_rules {
    use super::*;

    /// Ensure we handle both serializing and deserializing RuleViolationKind
    #[test]
    fn test_parsing_rule_kind() {
        let rule_names = RULES.iter().map(|r| r.name.clone());
        for rule in rule_names {
            assert_eq!(
                RuleViolationKind::try_from(rule.to_string().as_ref()),
                Ok(rule)
            );
        }
    }

    /// Ensure we stort the resulting violations by where they occur in the file.
    #[test]
    fn test_check_rules_orderin() {
        let sql = r#"
  ALTER TABLE "table_name" RENAME COLUMN "column_name" TO "new_column_name";
  CREATE INDEX "field_name_idx" ON "table_name" ("field_name");
  "#;

        let res = check_sql(sql, &["prefer-robust-stmts".into()]).expect("valid parsing of SQL");
        let mut prev_span_start = -1;
        for violation in res.iter() {
            assert!(violation.span.start > prev_span_start);
            prev_span_start = violation.span.start;
        }
    }
}