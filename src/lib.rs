//! Deoxys-II-256-128 MRAE primitives implementation.
#![cfg_attr(not(test), no_std)]
#![feature(alloc, test)]

#[cfg(not(all(target_feature = "aes", target_feature = "ssse3",)))]
compile_error!("The following target_feature flags must be set: +aes,+ssse3.");

extern crate alloc;
#[macro_use]
extern crate failure;

use alloc::vec::Vec;
use core::arch::x86_64::{
    __m128i, _mm_aesenc_si128, _mm_and_si128, _mm_load_si128, _mm_loadu_si128, _mm_or_si128,
    _mm_set1_epi8, _mm_set_epi64x, _mm_set_epi8, _mm_shuffle_epi8, _mm_slli_epi64, _mm_srli_epi64,
    _mm_store_si128, _mm_storeu_si128, _mm_xor_si128,
};

use subtle::ConstantTimeEq as _;
use zeroize::{Zeroize, ZeroizeOnDrop};

include!("constants.rs");
include!("primitives.rs");

#[derive(Debug, Fail)]
pub enum DecryptionError {
    #[fail(display = "Ciphertext did not include a complete tag.")]
    MissingTag,
    #[fail(display = "Tag verification failed")]
    InvalidTag,
}

/// Deoxys-II-256-128 state.
///
/// We don't store the key itself, but only components derived from the key.
/// These components are automatically erased after the structure is dropped.
#[derive(ZeroizeOnDrop)]
#[repr(align(16))]
pub struct DeoxysII {
    /// Derived K components for the sub-tweak keys for each round.
    /// These are derived from the key.
    derived_ks: [[u8; STK_SIZE]; STK_COUNT],
}

impl Zeroize for DeoxysII {
    /// Make sure the derived K components are erased before the struct
    /// is dropped, as they contain sensitive information.
    fn zeroize(&mut self) {
        for i in 0..STK_COUNT {
            self.derived_ks[i].zeroize();
        }
    }
}

macro_rules! process_blocks {
    (
        $input:ident,
        |$full_blocks:ident, $num_bytes:ident| $handle_full:block,
        |$full_blocks_:ident, $remaining_bytes:ident, $trailing_block:ident| $handle_trailing:block
    ) => {
        let $full_blocks = $input.len() / BLOCK_SIZE;
        let mut $remaining_bytes = $input.len();
        if $input.len() >= BLOCK_SIZE {
            let $num_bytes = $full_blocks * BLOCK_SIZE;
            $handle_full;
            $remaining_bytes -= $num_bytes;
        }
        if $remaining_bytes > 0 {
            let mut $trailing_block = [0u8; BLOCK_SIZE];
            $trailing_block[..$remaining_bytes]
                .copy_from_slice(&$input[$input.len() - $remaining_bytes..]);
            $handle_trailing;
        }
    };
}

impl DeoxysII {
    /// Creates a new instance using the provided `key`.
    pub fn new(key: &[u8; KEY_SIZE]) -> Self {
        Self {
            derived_ks: stk_derive_k(key),
        }
    }

    /// Encrypts and authenticates plaintext, authenticates the additional
    /// data and returns the result.
    pub fn seal(
        &self,
        nonce: &[u8; NONCE_SIZE],
        plaintext: Vec<u8>,
        additional_data: Vec<u8>,
    ) -> Vec<u8> {
        let mut auth = [0u8; TAG_SIZE];

        self.seal_ad(&additional_data, &mut auth);
        self.seal_message(&plaintext, &mut auth);

        // Handle nonce.
        let mut enc_nonce = [0u8; BLOCK_SIZE];
        enc_nonce[1..].copy_from_slice(nonce);
        enc_nonce[0] = PREFIX_TAG << PREFIX_SHIFT;
        bc_encrypt_in_place(&mut auth, &self.derived_ks, &enc_nonce);

        let pt_len = plaintext.len();
        let mut ciphertext = Vec::with_capacity(pt_len + TAG_SIZE);
        unsafe { ciphertext.set_len(ciphertext.capacity()) };

        // Put the tag at the end.
        ciphertext[pt_len..pt_len + TAG_SIZE].copy_from_slice(&auth);

        // Encrypt message.
        enc_nonce[0] = 0;

        // encode_enc_tweak() requires the first byte of the tag to be modified.
        auth[0] |= 0x80;

        self.seal_tag(&plaintext, &enc_nonce, &auth, &mut ciphertext);

        ciphertext
    }

    fn seal_ad(&self, additional_data: &[u8], auth: &mut [u8; 16]) {
        process_blocks!(
            additional_data,
            |full_blocks, num_bytes| {
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_AD_BLOCK,
                    0,
                    &additional_data[0..full_blocks * BLOCK_SIZE],
                    full_blocks,
                );
            },
            |full_blocks, remaining_bytes, astar| {
                astar[remaining_bytes] = 0x80;
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_AD_FINAL,
                    full_blocks,
                    &astar,
                    1,
                );
            }
        );
    }

    fn seal_message(&self, plaintext: &[u8], auth: &mut [u8; 16]) {
        process_blocks!(
            plaintext,
            |full_blocks, num_bytes| {
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_MSG_BLOCK,
                    0,
                    &plaintext[0..num_bytes],
                    full_blocks,
                );
            },
            |full_blocks, remaining_bytes, mstar| {
                mstar[remaining_bytes] = 0x80;
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_MSG_FINAL,
                    full_blocks,
                    &mstar,
                    1,
                );
            }
        );
    }

    fn seal_tag(&self, plaintext: &[u8], nonce: &[u8; 16], auth: &[u8; 16], ciphertext: &mut [u8]) {
        process_blocks!(
            plaintext,
            |full_blocks, num_bytes| {
                bc_xor_blocks(
                    &mut ciphertext[0..num_bytes],
                    &self.derived_ks,
                    &auth,
                    0,
                    &nonce,
                    &plaintext[0..num_bytes],
                    full_blocks,
                );
            },
            |full_blocks, remaining_bytes, trailing_block| {
                let mut out = [0u8; BLOCK_SIZE];
                bc_xor_blocks(
                    &mut out,
                    &self.derived_ks,
                    &auth,
                    full_blocks,
                    &nonce,
                    &trailing_block,
                    1,
                );
                let pt_len = plaintext.len();
                ciphertext[pt_len - remaining_bytes..pt_len]
                    .copy_from_slice(&out[..remaining_bytes]);
            }
        );
    }

    /// Decrypts and authenticates ciphertext, authenticates the additional
    /// data and, if successful, returns the resulting plaintext.
    /// If the tag verification fails, an error is returned and the
    /// intermediary plaintext is securely erased from memory.
    pub fn open(
        &self,
        nonce: &[u8; NONCE_SIZE],
        mut ciphertext_with_tag: Vec<u8>,
        additional_data: Vec<u8>,
    ) -> Result<Vec<u8>, DecryptionError> {
        let ctt_len = ciphertext_with_tag.len();
        if ctt_len < TAG_SIZE {
            return Err(DecryptionError::MissingTag);
        }

        let (ciphertext, tag) = ciphertext_with_tag.split_at_mut(ctt_len - TAG_SIZE);

        let mut plaintext = Vec::with_capacity(ciphertext.len());
        unsafe { plaintext.set_len(plaintext.capacity()) };

        let mut auth = [0u8; TAG_SIZE];

        let mut dec_nonce = self.open_message(&ciphertext, &tag, nonce, &mut plaintext);
        self.open_ad(&additional_data, &mut auth);
        self.open_tag(&plaintext, &mut auth);

        // tag' <- Ek(0001||0000||N, tag')
        dec_nonce[0] = PREFIX_TAG << PREFIX_SHIFT;
        bc_encrypt_in_place(&mut auth, &self.derived_ks, &dec_nonce);

        // Verify tag.
        let tags_are_equal = tag.ct_eq(&auth);
        if tags_are_equal.unwrap_u8() == 0 {
            plaintext.zeroize();
            tag.zeroize();
            auth.zeroize();
            Err(DecryptionError::InvalidTag)
        } else {
            Ok(plaintext)
        }
    }

    fn open_message(
        &self,
        ciphertext: &[u8],
        tag: &[u8],
        nonce: &[u8],
        plaintext: &mut [u8],
    ) -> [u8; BLOCK_SIZE] {
        let mut dec_nonce = [0u8; BLOCK_SIZE];
        let mut dec_tag = [0u8; TAG_SIZE];

        dec_nonce[1..].copy_from_slice(nonce);
        dec_tag.copy_from_slice(&tag);
        dec_tag[0] |= 0x80;

        process_blocks!(
            ciphertext,
            |full_blocks, num_bytes| {
                bc_xor_blocks(
                    &mut plaintext[0..num_bytes],
                    &self.derived_ks,
                    &dec_tag,
                    0,
                    &dec_nonce,
                    &ciphertext[0..num_bytes],
                    full_blocks,
                );
            },
            |full_blocks, remaining_bytes, trailing_block| {
                let mut out = [0u8; BLOCK_SIZE];
                bc_xor_blocks(
                    &mut out,
                    &self.derived_ks,
                    &dec_tag,
                    full_blocks,
                    &dec_nonce,
                    &trailing_block,
                    1,
                );
                plaintext[ciphertext.len() - remaining_bytes..ciphertext.len()]
                    .copy_from_slice(&out[..remaining_bytes]);
            }
        );

        dec_nonce
    }

    fn open_ad(&self, additional_data: &[u8], auth: &mut [u8; TAG_SIZE]) {
        process_blocks!(
            additional_data,
            |full_blocks, num_bytes| {
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_AD_BLOCK,
                    0,
                    &additional_data[0..num_bytes],
                    full_blocks,
                );
            },
            |full_blocks, remaining_bytes, astar| {
                astar[remaining_bytes] = 0x80;
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_AD_FINAL,
                    full_blocks,
                    &astar,
                    1,
                );
            }
        );
    }

    fn open_tag(&self, plaintext: &[u8], auth: &mut [u8; 16]) {
        process_blocks!(
            plaintext,
            |full_blocks, num_bytes| {
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_MSG_BLOCK,
                    0,
                    &plaintext[0..num_bytes],
                    full_blocks,
                );
            },
            |full_blocks, remaining_bytes, mstar| {
                mstar[remaining_bytes] = 0x80;
                accumulate_blocks(
                    auth,
                    &self.derived_ks,
                    PREFIX_MSG_FINAL,
                    full_blocks,
                    &mstar,
                    1,
                );
            }
        );
    }
}

include!("tests.rs");
