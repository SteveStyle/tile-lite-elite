use crate::model::{Letter, Position, Premium};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardState {
    pub cells: [BoardCell; 225],
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            cells: [BoardCell::Empty(EmptyCell {
                premium: Premium::Blank,
            }); 225],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardCell {
    Empty(EmptyCell),
    Filled(FilledCell),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyCell {
    pub premium: Premium,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilledCell {
    pub letter: Letter,
    pub is_blank: bool,
}

impl BoardState {
    pub const WIDTH: usize = 15;
    pub const HEIGHT: usize = 15;

    pub fn new(rules: &crate::model::VariantRules) -> Self {
        let mut cells = [BoardCell::Empty(EmptyCell {
            premium: Premium::Blank,
        }); 225];

        for (index, premium) in rules.premiums.iter().copied().enumerate() {
            cells[index] = BoardCell::Empty(EmptyCell { premium });
        }

        Self { cells }
    }

    pub fn get(&self, pos: Position) -> Option<&BoardCell> {
        let index = pos.to_index(Self::WIDTH);
        self.cells.get(index)
    }

    pub fn get_mut(&mut self, pos: Position) -> Option<&mut BoardCell> {
        let index = pos.to_index(Self::WIDTH);
        self.cells.get_mut(index)
    }

    pub fn set(&mut self, pos: Position, cell: BoardCell) {
        let index = pos.to_index(Self::WIDTH);
        self.cells[index] = cell;
    }

    pub fn filled_letter(&self, pos: Position) -> Option<(Letter, bool)> {
        match self.get(pos) {
            Some(BoardCell::Filled(cell)) => Some((cell.letter, cell.is_blank)),
            _ => None,
        }
    }
}
