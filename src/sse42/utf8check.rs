use crate::*;
#[cfg(target_arch = "x86")]
use std::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::utf8check::*;

macro_rules! initial_mins {
    () => {
        _mm_setr_epi8(
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128, // 10xx => false
            static_cast_i8!(0xC2_u8),
            -128,                     // 110x
            static_cast_i8!(0xE1_u8), // 1110
            static_cast_i8!(0xF1_u8), // 1111
        )
    };
}

macro_rules! second_mins {
    () => {
        _mm_setr_epi8(
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128,
            -128, // 10xx => false
            127,
            127,                      // 110x => true
            static_cast_i8!(0xA0_u8), // 1110
            static_cast_i8!(0x90_u8), // 1111
        )
    };
}

impl Default for ProcessedUtfBytes<__m128i> {
    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn default() -> Self {
        unsafe {
            Self {
                rawbytes: _mm_setzero_si128(),
                high_nibbles: _mm_setzero_si128(),
                carried_continuations: _mm_setzero_si128(),
            }
        }
    }
}

impl Utf8Check<__m128i> for ProcessedUtfBytes<__m128i> {
    #[cfg_attr(not(feature = "no-inline"), inline(always))]
    fn new_processed_utf_bytes() -> Self {
        Self::default()
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn push_last_byte_of_a_to_b(a: __m128i, b: __m128i) -> __m128i {
        unsafe { _mm_alignr_epi8(b, a, 15) }
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn push_last_2bytes_of_a_to_b(a: __m128i, b: __m128i) -> __m128i {
        unsafe { _mm_alignr_epi8(b, a, 14) }
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn check_smaller_than_0xf4(current_bytes: __m128i, has_error: &mut __m128i) {
        // unsigned, saturates to 0 below max
        *has_error = unsafe {
            _mm_or_si128(
                *has_error,
                _mm_subs_epu8(current_bytes, _mm_set1_epi8(static_cast_i8!(0xF4_u8))),
            )
        };
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn continuation_lengths(high_nibbles: __m128i) -> __m128i {
        unsafe {
            _mm_shuffle_epi8(
                _mm_setr_epi8(
                    1, 1, 1, 1, 1, 1, 1, 1, // 0xxx (ASCII)
                    0, 0, 0, 0, // 10xx (continuation)
                    2, 2, // 110x
                    3, // 1110
                    4, // 1111, next should be 0 (not checked here)
                ),
                high_nibbles,
            )
        }
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn carry_continuations(initial_lengths: __m128i, previous_carries: __m128i) -> __m128i {
        unsafe {
            let right1: __m128i = _mm_subs_epu8(
                Self::push_last_byte_of_a_to_b(previous_carries, initial_lengths),
                _mm_set1_epi8(1),
            );
            let sum: __m128i = _mm_add_epi8(initial_lengths, right1);
            let right2: __m128i = _mm_subs_epu8(
                Self::push_last_2bytes_of_a_to_b(previous_carries, sum),
                _mm_set1_epi8(2),
            );
            _mm_add_epi8(sum, right2)
        }
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn check_continuations(initial_lengths: __m128i, carries: __m128i, has_error: &mut __m128i) {
        unsafe {
            // overlap || underlap
            // carry > length && length > 0 || !(carry > length) && !(length > 0)
            // (carries > length) == (lengths > 0)
            let overunder: __m128i = _mm_cmpeq_epi8(
                _mm_cmpgt_epi8(carries, initial_lengths),
                _mm_cmpgt_epi8(initial_lengths, _mm_setzero_si128()),
            );

            *has_error = _mm_or_si128(*has_error, overunder);
        }
    }

    // when 0xED is found, next byte must be no larger than 0x9F
    // when 0xF4 is found, next byte must be no larger than 0x8F
    // next byte must be continuation, ie sign bit is set, so signed < is ok
    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn check_first_continuation_max(
        current_bytes: __m128i,
        off1_current_bytes: __m128i,
        has_error: &mut __m128i,
    ) {
        unsafe {
            let mask_ed: __m128i =
                _mm_cmpeq_epi8(off1_current_bytes, _mm_set1_epi8(static_cast_i8!(0xED_u8)));
            let mask_f4: __m128i =
                _mm_cmpeq_epi8(off1_current_bytes, _mm_set1_epi8(static_cast_i8!(0xF4_u8)));

            let badfollow_ed: __m128i = _mm_and_si128(
                _mm_cmpgt_epi8(current_bytes, _mm_set1_epi8(static_cast_i8!(0x9F_u8))),
                mask_ed,
            );
            let badfollow_f4: __m128i = _mm_and_si128(
                _mm_cmpgt_epi8(current_bytes, _mm_set1_epi8(static_cast_i8!(0x8F_u8))),
                mask_f4,
            );

            *has_error = _mm_or_si128(*has_error, _mm_or_si128(badfollow_ed, badfollow_f4));
        }
    }

    // map off1_hibits => error condition
    // hibits     off1    cur
    // C       => < C2 && true
    // E       => < E1 && < A0
    // F       => < F1 && < 90
    // else      false && false
    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn check_overlong(
        current_bytes: __m128i,
        off1_current_bytes: __m128i,
        hibits: __m128i,
        previous_hibits: __m128i,
        has_error: &mut __m128i,
    ) {
        unsafe {
            let off1_hibits: __m128i = Self::push_last_byte_of_a_to_b(previous_hibits, hibits);
            let initial_mins: __m128i = _mm_shuffle_epi8(initial_mins!(), off1_hibits);

            let initial_under: __m128i = _mm_cmpgt_epi8(initial_mins, off1_current_bytes);

            let second_mins: __m128i = _mm_shuffle_epi8(second_mins!(), off1_hibits);
            let second_under: __m128i = _mm_cmpgt_epi8(second_mins, current_bytes);
            *has_error = _mm_or_si128(*has_error, _mm_and_si128(initial_under, second_under));
        }
    }

    #[cfg_attr(not(feature = "no-inline"), inline)]
    fn count_nibbles(bytes: __m128i, answer: &mut Self) {
        answer.rawbytes = bytes;
        answer.high_nibbles =
            unsafe { _mm_and_si128(_mm_srli_epi16(bytes, 4), _mm_set1_epi8(0x0F)) };
    }
}
