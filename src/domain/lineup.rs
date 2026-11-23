// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Lineup of a session: who is selected, and the pairs of each game.
//!
//! The schema keeps one `lineup_entries` row per selected player, with at most one game and
//! team, so a player plays in one game of a session at most.

use core::fmt;
use std::collections::{BTreeMap, HashSet};

use super::ids::{GuestId, MemberId};

/// A player of a lineup: a member or a guest.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Seat {
    Member(MemberId),
    Guest(GuestId),
}

/// Team of a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Team {
    A,
    B,
}

impl Team {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

/// A `lineup_entries` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Rank in the selection, from 1.
    pub(crate) position: i64,
    pub(crate) seat: Seat,
    /// `None`: selected, waits for the next game.
    pub(crate) game: Option<(i64, Team)>,
}

/// One game: two teams of two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Game {
    pub(crate) number: i64,
    pub(crate) a: Vec<Seat>,
    pub(crate) b: Vec<Seat>,
}

/// A lineup that breaks a rule: the API answers 422 with this text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineupError {
    TwiceInSelection,
    TooManySelected { places: u32 },
    InvalidGameNumber,
    TeamOfTwo { game: i64 },
    NotSelected { game: i64 },
    InTwoPlaces { game: i64 },
}

impl fmt::Display for LineupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TwiceInSelection => f.write_str("a player appears twice in the selection"),
            Self::TooManySelected { places } => {
                write!(
                    f,
                    "the selection exceeds the {places} places of the session"
                )
            }
            Self::InvalidGameNumber => f.write_str("game numbers are distinct and start at 1"),
            Self::TeamOfTwo { game } => write!(f, "each team of game {game} has exactly 2 players"),
            Self::NotSelected { game } => {
                write!(f, "a player of game {game} is not in the selection")
            }
            Self::InTwoPlaces { game } => write!(
                f,
                "a player of game {game} already plays in another game or team"
            ),
        }
    }
}

impl std::error::Error for LineupError {}

/// The entries of a selection and its games, checked against the effective places.
pub(crate) fn entries(
    selection: &[Seat],
    games: &[Game],
    places: u32,
) -> Result<Vec<Entry>, LineupError> {
    let mut selected = HashSet::new();
    if !selection.iter().all(|seat| selected.insert(seat)) {
        return Err(LineupError::TwiceInSelection);
    }
    if selection.len() > usize::try_from(places).unwrap_or(usize::MAX) {
        return Err(LineupError::TooManySelected { places });
    }
    let mut numbers = HashSet::new();
    let mut assigned = BTreeMap::new();
    for game in games {
        if game.number < 1 || !numbers.insert(game.number) {
            return Err(LineupError::InvalidGameNumber);
        }
        for (team, seats) in [(Team::A, &game.a), (Team::B, &game.b)] {
            if seats.len() != 2 {
                return Err(LineupError::TeamOfTwo { game: game.number });
            }
            for seat in seats {
                if !selected.contains(seat) {
                    return Err(LineupError::NotSelected { game: game.number });
                }
                if assigned.insert(seat, (game.number, team)).is_some() {
                    return Err(LineupError::InTwoPlaces { game: game.number });
                }
            }
        }
    }
    Ok(selection
        .iter()
        .zip(1..)
        .map(|(seat, position)| Entry {
            position,
            seat: seat.clone(),
            game: assigned.get(seat).copied(),
        })
        .collect())
}

/// The selection and the games of stored entries, games by number.
pub(crate) fn split(entries: &[Entry]) -> (Vec<Seat>, Vec<Game>) {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by_key(|entry| entry.position);
    let selection = sorted.iter().map(|entry| entry.seat.clone()).collect();
    let mut games: BTreeMap<i64, Game> = BTreeMap::new();
    for entry in sorted {
        if let Some((number, team)) = entry.game {
            let game = games.entry(number).or_insert_with(|| Game {
                number,
                a: Vec::new(),
                b: Vec::new(),
            });
            match team {
                Team::A => game.a.push(entry.seat.clone()),
                Team::B => game.b.push(entry.seat.clone()),
            }
        }
    }
    (selection, games.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::{Entry, Game, LineupError, Seat, Team, entries, split};
    use crate::domain::ids::{GuestId, MemberId};

    fn member(n: u8) -> Seat {
        let id = format!("m_{}", "a".repeat(25) + &char::from(b'a' + n).to_string());
        Seat::Member(MemberId::parse(&id).expect("should be a member id"))
    }

    fn guest() -> Seat {
        Seat::Guest(GuestId::parse(&format!("g_{}", "b".repeat(26))).expect("should be a guest id"))
    }

    fn game(number: i64, a: &[Seat; 2], b: &[Seat; 2]) -> Game {
        Game {
            number,
            a: a.to_vec(),
            b: b.to_vec(),
        }
    }

    #[test]
    fn selection_and_games_become_entries_and_back() {
        let selection = [
            member(1),
            member(2),
            member(3),
            member(4),
            guest(),
            member(5),
        ];
        let games = [game(1, &[member(1), member(3)], &[member(2), guest()])];
        let written = entries(&selection, &games, 6).expect("should accept the lineup");
        assert_eq!(
            written[0],
            Entry {
                position: 1,
                seat: member(1),
                game: Some((1, Team::A))
            },
            "the first selected player plays game 1 in team a"
        );
        assert_eq!(
            written[3].game, None,
            "member 4 is selected and waits for the next game"
        );
        assert_eq!(
            split(&written),
            (selection.to_vec(), games.to_vec()),
            "stored entries give the same lineup back"
        );
    }

    #[test]
    fn rules_of_a_lineup() {
        let four = [member(1), member(2), member(3), member(4)];
        for (selection, games, places, expected) in [
            (
                vec![member(1), member(1)],
                vec![],
                4,
                LineupError::TwiceInSelection,
            ),
            (
                four.to_vec(),
                vec![],
                3,
                LineupError::TooManySelected { places: 3 },
            ),
            (
                four.to_vec(),
                vec![game(0, &[member(1), member(2)], &[member(3), member(4)])],
                4,
                LineupError::InvalidGameNumber,
            ),
            (
                four.to_vec(),
                vec![Game {
                    number: 1,
                    a: vec![member(1)],
                    b: vec![member(3), member(4)],
                }],
                4,
                LineupError::TeamOfTwo { game: 1 },
            ),
            (
                four.to_vec(),
                vec![game(1, &[member(1), member(2)], &[member(3), member(5)])],
                4,
                LineupError::NotSelected { game: 1 },
            ),
            (
                four.to_vec(),
                vec![game(1, &[member(1), member(2)], &[member(3), member(1)])],
                4,
                LineupError::InTwoPlaces { game: 1 },
            ),
            (
                four.to_vec(),
                vec![
                    game(1, &[member(1), member(2)], &[member(3), member(4)]),
                    game(2, &[member(1), member(2)], &[member(3), member(4)]),
                ],
                4,
                LineupError::InTwoPlaces { game: 2 },
            ),
        ] {
            assert_eq!(
                entries(&selection, &games, places),
                Err(expected.clone()),
                "{expected}"
            );
        }
    }

    #[test]
    fn an_empty_lineup_is_valid() {
        assert_eq!(entries(&[], &[], 4), Ok(vec![]), "nobody selected yet");
    }
}
