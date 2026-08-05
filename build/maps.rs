use crate::{attacks::*, magics::*};

pub fn generate_king_map() -> [u64; 64] {
    generate_map(king_attacks)
}

pub fn generate_knight_map() -> [u64; 64] {
    generate_map(knight_attacks)
}

fn generate_map<F: Fn(u8) -> u64>(f: F) -> [u64; 64] {
    let mut map = [0; 64];
    for square in 0..64 {
        map[square as usize] = f(square as u8);
    }

    map
}

pub fn generate_between_map() -> [[u64; 64]; 64] {
    std::array::from_fn(|i| generate_map(|square| generate_ray(square, i as u8, true)))
}

pub fn generate_rays_map() -> [[u64; 64]; 64] {
    std::array::from_fn(|i| generate_map(|square| generate_ray(square, i as u8, false)))
}

pub fn generate_pawn_map() -> [[u64; 64]; 2] {
    [
        generate_map(|square| pawn_attacks(square, Color::White)),
        generate_map(|square| pawn_attacks(square, Color::Black)),
    ]
}

pub fn generate_diagonal_tables() -> [[u64; 64]; 2] {
    [
        generate_map(|square| sliding_attacks(square, 0, &[9, -9])),
        generate_map(|square| sliding_attacks(square, 0, &[7, -7])),
    ]
}

const SLIDERS_LEN: usize = 77519;

pub fn generate_sliding_map() -> Vec<u64> {
    let mut map = vec![0; SLIDERS_LEN];
    init_sliding_map(&ROOK_MAGICS, &[8, -8, 1, -1], &mut map);
    init_sliding_map(&BISHOP_MAGICS, &[9, 7, -7, -9], &mut map);
    map
}

fn init_sliding_map(magics: &[MagicEntry], directions: &[i8], map: &mut [u64]) {
    for square in 0..64 {
        let entry = &magics[square as usize];

        let mut occupancies = 0u64;
        loop {
            let hash = magic_index(occupancies, entry);
            map[hash] = sliding_attacks(square, occupancies, directions);

            occupancies = occupancies.wrapping_sub(!entry.mask) & !entry.mask;
            if occupancies == 0 {
                break;
            }
        }
    }
}

const fn magic_index(occupancies: u64, entry: &MagicEntry) -> usize {
    let mut hash = occupancies | entry.mask;
    hash = hash.wrapping_mul(entry.magic) >> entry.shift;
    (hash as usize).wrapping_add_signed(entry.offset)
}
