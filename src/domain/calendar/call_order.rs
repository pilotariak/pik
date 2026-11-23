// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Order in which players are called for a session, and sessions played this season.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use jiff::civil::Date;

use super::session::Status;

/// One answer of a session: an `answers` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Answer<'a> {
    pub(crate) member_id: &'a str,
    pub(crate) status: Status,
    /// Changes only when the status changes.
    pub(crate) answered_at: Timestamp,
}

/// Members of a session in the order they are called. The first `places` take the places.
///
/// OK answers come first, the earliest first. The remaining places go to the Dispo answers: the
/// member with the fewest sessions played this season first (`played`, from
/// [`sessions_played`]), then the earliest answer. The member id breaks the last ties, so the
/// order never depends on the order of the rows. KO answers are left out.
pub(crate) fn call_order<'a>(answers: &[Answer<'a>], played: &HashMap<&str, u32>) -> Vec<&'a str> {
    let mut called: Vec<&Answer<'a>> = answers
        .iter()
        .filter(|answer| answer.status != Status::Ko)
        .collect();
    called.sort_by_key(|answer| {
        let dispo = answer.status == Status::Dispo;
        let played = if dispo {
            played.get(answer.member_id).copied().unwrap_or(0)
        } else {
            0
        };
        (dispo, played, answer.answered_at, answer.member_id)
    });
    called.into_iter().map(|answer| answer.member_id).collect()
}

/// A `lineup_entries` row, reduced to what counts for sessions played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineupEntry<'a> {
    pub(crate) date: Date,
    /// `None` for a guest.
    pub(crate) member_id: Option<&'a str>,
    /// `None`: selected, waits for the next game. That member still counts as having played.
    pub(crate) game: Option<i64>,
}

/// Sessions played this season by each member: the number of distinct dates of their lineup
/// entries that are before `today` and not `cancelled`.
///
/// `today` is the Paris date of now ([`paris_date`](super::paris_date)): a session counts from
/// the day after it. A slot covers one season, so there is no season filter. A member who has
/// not played yet is absent from the result: count them as 0.
pub(crate) fn sessions_played<'a>(
    entries: &[LineupEntry<'a>],
    cancelled: &HashSet<Date>,
    today: Date,
) -> HashMap<&'a str, u32> {
    let played: HashSet<(&'a str, Date)> = entries
        .iter()
        .filter(|entry| entry.date < today && !cancelled.contains(&entry.date))
        .filter_map(|entry| Some((entry.member_id?, entry.date)))
        .collect();
    let mut counts = HashMap::new();
    for (member_id, _) in played {
        *counts.entry(member_id).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use jiff::Timestamp;
    use jiff::civil::date;

    use super::{Answer, LineupEntry, call_order, sessions_played};
    use crate::domain::calendar::session::Status;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn answer<'a>(member_id: &'a str, status: Status, answered_at: &str) -> Answer<'a> {
        Answer {
            member_id,
            status,
            answered_at: at(answered_at),
        }
    }

    #[test]
    fn ok_answers_first_then_dispo_by_fewest_sessions_played() {
        let answers = [
            answer("dispo-5-played", Status::Dispo, "2027-03-15T09:00:00Z"),
            answer("ok-second", Status::Ok, "2027-03-15T10:05:00Z"),
            answer("dispo-2-played-late", Status::Dispo, "2027-03-15T11:00:00Z"),
            answer("ko", Status::Ko, "2027-03-15T08:00:00Z"),
            answer("ok-first", Status::Ok, "2027-03-15T10:00:00Z"),
            answer("dispo-never-played", Status::Dispo, "2027-03-15T12:00:00Z"),
            answer(
                "dispo-2-played-early",
                Status::Dispo,
                "2027-03-15T10:30:00Z",
            ),
        ];
        let played = HashMap::from([
            ("dispo-5-played", 5),
            ("dispo-2-played-late", 2),
            ("dispo-2-played-early", 2),
            ("ok-first", 9),
        ]);
        assert_eq!(
            call_order(&answers, &played),
            [
                "ok-first",
                "ok-second",
                "dispo-never-played",
                "dispo-2-played-early",
                "dispo-2-played-late",
                "dispo-5-played",
            ],
            "OK by answer time, then Dispo by sessions played and answer time, without KO"
        );
    }

    #[test]
    fn an_ok_answer_goes_before_any_dispo_answer() {
        let answers = [
            answer("dispo", Status::Dispo, "2027-03-15T08:00:00Z"),
            answer("ok", Status::Ok, "2027-03-15T20:00:00Z"),
        ];
        let played = HashMap::from([("ok", 12)]);
        assert_eq!(
            call_order(&answers, &played),
            ["ok", "dispo"],
            "the OK answer wins over an earlier Dispo answer whose member never played"
        );
    }

    #[test]
    fn order_does_not_depend_on_the_order_of_the_rows() {
        let mut answers = [
            answer("b", Status::Dispo, "2027-03-15T10:00:00Z"),
            answer("a", Status::Dispo, "2027-03-15T10:00:00Z"),
            answer("d", Status::Ok, "2027-03-15T10:00:00Z"),
            answer("c", Status::Ok, "2027-03-15T10:00:00Z"),
        ];
        let played = HashMap::new();
        assert_eq!(
            call_order(&answers, &played),
            ["c", "d", "a", "b"],
            "the member id breaks the ties"
        );
        answers.reverse();
        assert_eq!(
            call_order(&answers, &played),
            ["c", "d", "a", "b"],
            "same order with the rows reversed"
        );
    }

    #[test]
    fn sessions_played_counts_distinct_past_dates_that_are_not_cancelled() {
        let entry = |date, member_id, game| LineupEntry {
            date,
            member_id,
            game,
        };
        let entries = [
            entry(date(2027, 3, 1), Some("ana"), Some(1)),
            entry(date(2027, 3, 1), Some("bixente"), Some(1)),
            // Selected, waiting for the next game: still played.
            entry(date(2027, 3, 1), Some("carmen"), None),
            // A guest.
            entry(date(2027, 3, 1), None, Some(1)),
            entry(date(2027, 3, 8), Some("ana"), Some(2)),
            // Cancelled date: the lineup stays, but nobody played.
            entry(date(2027, 3, 15), Some("ana"), Some(1)),
            entry(date(2027, 3, 15), Some("bixente"), Some(1)),
            // Today and later: not played yet.
            entry(date(2027, 3, 22), Some("ana"), Some(1)),
            entry(date(2027, 3, 29), Some("bixente"), None),
        ];
        let cancelled = HashSet::from([date(2027, 3, 15)]);
        assert_eq!(
            sessions_played(&entries, &cancelled, date(2027, 3, 22)),
            HashMap::from([("ana", 2), ("bixente", 1), ("carmen", 1)]),
            "past dates that are not cancelled, guests left out"
        );
        assert_eq!(
            sessions_played(&entries, &cancelled, date(2027, 3, 23)).get("ana"),
            Some(&3),
            "the next day, the session of 22 March counts"
        );
    }
}
