//extern crate num_traits;

//use num_traits::Num;
use std::fmt::{Debug, Display, Formatter};
use std::ops::AddAssign;
use std::str::FromStr;

use crate::Direction;

type PosIndex = u8;

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct Position {
    pub x: PosIndex,
    pub y: PosIndex,
}

impl Display for Position {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", (self.x as u8 + 'A' as u8) as char, self.y + 1)
    }
}

impl FromStr for Position {
    // expecting the format A10
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut chars = s.chars();
        if s.len() < 2 {
            return Err(format!("Invalid position: {}", s));
        }

        let column = chars.next().unwrap();
        match column {
            'A'..='O' => (),
            _ => return Err(format!("Invalid position: {}", s)),
        }
        let column = column as u8 - 'A' as u8;

        let mut row = chars.next().unwrap().to_string();
        if let Some(z) = chars.next() {
            row.push(z);
        }
        match row.parse::<u8>() {
            Ok(r) => Ok(Position {
                x: column,
                y: r - 1,
            }),

            Err(_) => Err(format!("Invalid position: {}", s)),
        }
    }
}

// implement add and subtract for position
impl std::ops::Add<Position> for Position {
    type Output = Position;
    fn add(self, other: Position) -> Position {
        Position {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl std::ops::Sub<Position> for Position {
    type Output = Position;
    fn sub(self, other: Position) -> Position {
        Position {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}
impl std::ops::Mul<PosIndex> for Position {
    type Output = Position;
    fn mul(self, other: PosIndex) -> Position {
        Position {
            x: self.x * other,
            y: self.y * other,
        }
    }
}
impl AddAssign for Position {
    fn add_assign(&mut self, other: Position) {
        *self = *self + other;
    }
}

// implement the new function for Position
impl Position {
    pub fn new(x: PosIndex, y: PosIndex) -> Position {
        Position { x, y }
    }

    pub fn range_to(&self, other: &Position) -> Vec<Position> {
        let mut result = vec![];
        if self.x > other.x || self.y > other.y {
            return result;
        }
        for x in self.x..=other.x {
            for y in self.y..=other.y {
                result.push(Position::new(x, y));
            }
        }
        result
    }

    pub fn try_step_forward(&self, direction: Direction) -> Option<Position> {
        match direction {
            Direction::Horizontal => {
                if self.x < 14 {
                    Some(Position::new(self.x + 1, self.y))
                } else {
                    None
                }
            }
            Direction::Vertical => {
                if self.y < 14 {
                    Some(Position::new(self.x, self.y + 1))
                } else {
                    None
                }
            }
        }
    }

    pub fn try_step_backward(&self, direction: Direction) -> Option<Position> {
        match direction {
            Direction::Horizontal => {
                if self.x > 0 {
                    Some(Position::new(self.x - 1, self.y))
                } else {
                    None
                }
            }
            Direction::Vertical => {
                if self.y > 0 {
                    Some(Position::new(self.x, self.y - 1))
                } else {
                    None
                }
            }
        }
    }
}

// test range_to
#[cfg(test)]
mod test {
    use crate::pos::Position;

    #[test]
    fn test_range_to() {
        let pos1 = Position::new(0, 0);
        let pos2 = Position::new(2, 2);
        let range = pos1.range_to(&pos2);
        assert_eq!(range.len(), 9);
        println!("{:?}", range);
    }
    #[test]
    fn test_size() {
        let pos1 = Position::new(0, 0);
        let pos2 = Position::new(2, 2);
        let pos3: Option<Position> = None;
        let pos4: Option<Position> = Some(Position::new(2, 2));
        let int5: Option<i32> = None;
        let int6: Option<i32> = Some(2);
        let result1: Result<(), Position> = Ok(());
        let result2: Result<(), Position> = Err(Position::new(2, 2));
        let result3: Result<Position, Position> = Ok(Position::new(2, 2));
        let result4: Result<Position, Position> = Err(Position::new(2, 2));

        println!("size of result1: {}", std::mem::size_of_val(&result1));
        println!("size of result2: {}", std::mem::size_of_val(&result2));
        println!("size of result3: {}", std::mem::size_of_val(&result3));
        println!("size of result4: {}", std::mem::size_of_val(&result4));
        println!("size of pos1: {}", std::mem::size_of_val(&pos1));
        println!("size of pos2: {}", std::mem::size_of_val(&pos2));
        println!("size of pos3: {}", std::mem::size_of_val(&pos3));
        println!("size of pos4: {}", std::mem::size_of_val(&pos4));
        println!("size of int5: {}", std::mem::size_of_val(&int5));
        println!("size of int6: {}", std::mem::size_of_val(&int6));
    }
}
