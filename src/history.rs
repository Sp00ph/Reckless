use std::{
    alloc::{self, Layout},
    ptr::NonNull,
    sync::atomic::{AtomicI16, Ordering},
};

use crate::types::{Bitboard, Color, Move, Piece, PieceType, Square};

type FromToHistory<T> = [[T; 64]; 64];
type PieceToHistory<T> = [[T; 64]; 13];
type ContinuationHistoryType = [[[[PieceToHistory<i16>; 64]; 13]; 2]; 2];

// Force 2MiB alignment to match hugepage aligment
const MIN_ALIGN: usize = 1 << 21;

struct HugeBox<T> {
    ptr: NonNull<T>,
}

unsafe impl<T: Send> Send for HugeBox<T> {}
unsafe impl<T: Sync> Sync for HugeBox<T> {}

impl<T> HugeBox<T> {
    const ALIGN: usize = if MIN_ALIGN >= align_of::<T>() { MIN_ALIGN } else { align_of::<T>() };
    const SIZE: usize = size_of::<T>().next_multiple_of(Self::ALIGN);
    // FIXME: Just .unwrap() once that works in const.
    const LAYOUT: Layout = match Layout::from_size_align(Self::SIZE, Self::ALIGN) {
        Ok(layout) => layout,
        Err(_) => unreachable!(),
    };

    fn new_zeroed() -> Self {
        let ptr = unsafe {
            let p = alloc::alloc_zeroed(Self::LAYOUT);
            if p.is_null() {
                alloc::handle_alloc_error(Self::LAYOUT);
            }
            #[cfg(target_os = "linux")]
            libc::madvise(p.cast(), Self::SIZE, libc::MADV_HUGEPAGE);
            NonNull::new_unchecked(p.cast::<T>())
        };

        HugeBox { ptr }
    }
}

impl<T> std::ops::Deref for HugeBox<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> std::ops::DerefMut for HugeBox<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for HugeBox<T> {
    fn drop(&mut self) {
        unsafe {
            alloc::dealloc(self.ptr.as_ptr().cast(), Self::LAYOUT);
        }
    }
}

fn apply_bonus<const MAX: i32>(entry: &mut i16, bonus: i32) {
    let bonus = bonus.clamp(-MAX, MAX);
    *entry += (bonus - bonus.abs() * (*entry) as i32 / MAX) as i16;
}

pub struct QuietHistory {
    // [side_to_move][from_threatened][to_threatened][from][to]
    entries: Box<[[[FromToHistory<i16>; 2]; 2]; 2]>,
}

impl QuietHistory {
    const MAX_HISTORY: i32 = 8192;

    pub fn get(&self, threats: Bitboard, stm: Color, mv: Move) -> i32 {
        self.entries[stm][threats.contains(mv.from()) as usize][threats.contains(mv.to()) as usize][mv.from()][mv.to()]
            as i32
    }

    pub fn update(&mut self, threats: Bitboard, stm: Color, mv: Move, bonus: i32) {
        let entry = &mut self.entries[stm][threats.contains(mv.from()) as usize][threats.contains(mv.to()) as usize]
            [mv.from()][mv.to()];
        apply_bonus::<{ Self::MAX_HISTORY }>(entry, bonus);
    }
}

impl Default for QuietHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

pub struct PawnHistory {
    // [pawn_key_bucket][piece][to]
    entries: Box<[PieceToHistory<i16>; Self::SIZE]>,
}

impl PawnHistory {
    const MAX_HISTORY: i32 = 8192;

    const SIZE: usize = 512;
    const MASK: usize = Self::SIZE - 1;

    pub fn get(&self, pawn_key: u64, piece: Piece, to: Square) -> i32 {
        self.entries[pawn_key as usize & Self::MASK][piece][to] as i32
    }

    pub fn update(&mut self, pawn_key: u64, piece: Piece, to: Square, bonus: i32) {
        let entry = &mut self.entries[pawn_key as usize & Self::MASK][piece][to];
        apply_bonus::<{ Self::MAX_HISTORY }>(entry, bonus);
    }
}

impl Default for PawnHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

pub struct NoisyHistory {
    // [piece][to][captured_piece_type][to_threatened]
    entries: Box<PieceToHistory<[[i16; 2]; 7]>>,
}

impl NoisyHistory {
    const MAX_HISTORY: i32 = 12800;

    pub fn get(&self, threats: Bitboard, piece: Piece, sq: Square, captured: PieceType) -> i32 {
        self.entries[piece][sq][captured][threats.contains(sq) as usize] as i32
    }

    pub fn update(&mut self, threats: Bitboard, piece: Piece, sq: Square, captured: PieceType, bonus: i32) {
        let entry = &mut self.entries[piece][sq][captured][threats.contains(sq) as usize];
        apply_bonus::<{ Self::MAX_HISTORY }>(entry, bonus);
    }
}

impl Default for NoisyHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

pub struct CorrectionHistory {
    // [bucket][side_to_move][key]
    entries: Box<[[[AtomicI16; Self::SIZE]; 2]; 16]>,
}

impl CorrectionHistory {
    const MAX_HISTORY: i32 = 14605;

    const SIZE: usize = 65536;
    const MASK: usize = Self::SIZE - 1;

    pub fn get(&self, stm: Color, key: u64, bucket: usize) -> i32 {
        self.entries[bucket][stm][key as usize & Self::MASK].load(Ordering::Relaxed) as i32
    }

    pub fn update(&self, stm: Color, key: u64, bucket: usize, bonus: i32) {
        let current = self.entries[bucket][stm][key as usize & Self::MASK].load(Ordering::Relaxed) as i32;
        let new = current + bonus - bonus.abs() * current / Self::MAX_HISTORY;
        self.entries[bucket][stm][key as usize & Self::MASK].store(new as i16, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        for bucket in self.entries.iter() {
            for entries in bucket.iter() {
                for entry in entries {
                    entry.store(0, Ordering::Relaxed);
                }
            }
        }
    }
}

impl Default for CorrectionHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

pub struct ContinuationCorrectionHistory {
    // [in_check][capture][piece][to][piece][to]
    entries: HugeBox<ContinuationHistoryType>,
}

impl ContinuationCorrectionHistory {
    const MAX_HISTORY: i32 = 16418;

    pub fn subtable_ptr(
        &mut self, in_check: bool, capture: bool, piece: Piece, to: Square,
    ) -> *mut PieceToHistory<i16> {
        &raw mut self.entries[in_check as usize][capture as usize][piece][to]
    }

    pub fn get(&self, subtable_ptr: *mut PieceToHistory<i16>, piece: Piece, to: Square) -> i32 {
        unsafe { (&*subtable_ptr)[piece][to] as i32 }
    }

    pub fn update(&self, subtable_ptr: *mut PieceToHistory<i16>, piece: Piece, to: Square, bonus: i32) {
        let entry = &mut unsafe { &mut *subtable_ptr }[piece][to];
        apply_bonus::<{ Self::MAX_HISTORY }>(entry, bonus);
    }
}

impl Default for ContinuationCorrectionHistory {
    fn default() -> Self {
        Self { entries: HugeBox::new_zeroed() }
    }
}

pub struct ContinuationHistory {
    // [in_check][capture][piece][to][piece][to]
    entries: HugeBox<ContinuationHistoryType>,
}

impl ContinuationHistory {
    const MAX_HISTORY: i32 = 15320;

    pub fn subtable_ptr(
        &mut self, in_check: bool, capture: bool, piece: Piece, to: Square,
    ) -> *mut PieceToHistory<i16> {
        &raw mut self.entries[in_check as usize][capture as usize][piece][to]
    }

    pub fn get(&self, subtable_ptr: *mut PieceToHistory<i16>, piece: Piece, to: Square) -> i32 {
        (unsafe { &*subtable_ptr }[piece][to]) as i32
    }

    pub fn update(&self, subtable_ptr: *mut PieceToHistory<i16>, piece: Piece, to: Square, bonus: i32) {
        let entry = &mut unsafe { &mut *subtable_ptr }[piece][to];
        apply_bonus::<{ Self::MAX_HISTORY }>(entry, bonus);
    }
}

impl Default for ContinuationHistory {
    fn default() -> Self {
        Self { entries: HugeBox::new_zeroed() }
    }
}

fn zeroed_box<T>() -> Box<T> {
    unsafe {
        let layout = Layout::new::<T>();
        let ptr = alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            alloc::handle_alloc_error(layout);
        }
        Box::<T>::from_raw(ptr.cast())
    }
}
