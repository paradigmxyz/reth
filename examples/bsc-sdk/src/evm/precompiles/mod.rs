use super::spec::BscSpecId;
use cfg_if::cfg_if;
use once_cell::{race::OnceBox, sync::Lazy};
use revm::{
    context::Cfg,
    context_interface::ContextTr,
    handler::{EthPrecompiles, PrecompileProvider},
    interpreter::InterpreterResult,
    precompile::{bls12_381, kzg_point_evaluation, Precompiles},
    primitives::{Address, Bytes},
};
#[cfg(feature = "secp256r1")]
use revm::precompile::secp256r1;
use std::boxed::Box;

mod bls;
mod cometbft;
mod double_sign;
mod error;
mod iavl;
mod tendermint;
#[cfg(feature = "secp256k1")]
mod tm_secp256k1;

// BSC precompile provider
#[derive(Debug, Clone)]
pub struct BscPrecompiles {
    /// Inner precompile provider is same as Ethereums.
    inner: EthPrecompiles,
}

impl BscPrecompiles {
    /// Create a new [`BscPrecompiles`] with the given precompiles.
    pub fn new(precompiles: &'static Precompiles) -> Self {
        Self { inner: EthPrecompiles { precompiles } }
    }

    /// Create a new precompile provider with the given bsc spec.
    #[inline]
    pub fn new_with_spec(spec: BscSpecId) -> Self {
        match spec {
            // Pre-BSC hardforks use standard Ethereum precompiles
            BscSpecId::FRONTIER | BscSpecId::FRONTIER_THAWING | BscSpecId::HOMESTEAD |
            BscSpecId::TANGERINE | BscSpecId::SPURIOUS_DRAGON => {
                Self::new(Precompiles::homestead())
            }
            BscSpecId::BYZANTIUM | BscSpecId::CONSTANTINOPLE | BscSpecId::PETERSBURG => {
                Self::new(Precompiles::byzantium())
            }
            BscSpecId::ISTANBUL | BscSpecId::MUIR_GLACIER => Self::new(Precompiles::istanbul()),
            // BSC specific hardforks
            BscSpecId::RAMANUJAN | BscSpecId::NIELS | BscSpecId::MIRROR_SYNC | BscSpecId::BRUNO | BscSpecId::EULER => {
                Self::new(istanbul())
            }
            BscSpecId::NANO => Self::new(nano()),
            BscSpecId::MORAN | BscSpecId::GIBBS => Self::new(moran()),
            BscSpecId::PLANCK => Self::new(planck()),
            BscSpecId::LUBAN => Self::new(luban()),
            BscSpecId::PLATO => Self::new(plato()),
            BscSpecId::BERLIN | BscSpecId::LONDON | BscSpecId::SHANGHAI => Self::new(Precompiles::berlin()),
            BscSpecId::HERTZ | BscSpecId::HERTZ_FIX | BscSpecId::KEPLER => Self::new(hertz()),
            BscSpecId::FEYNMAN | BscSpecId::FEYNMAN_FIX => Self::new(feynman()),
            BscSpecId::HABER | BscSpecId::HABER_FIX | BscSpecId::BOHR => Self::new(haber()),
            BscSpecId::CANCUN => Self::new(cancun()),
            BscSpecId::LATEST => Self::new(latest()),
        }
    }
}

/// Returns precompiles for Istanbul spec.
pub fn istanbul() -> &'static Precompiles {
    static ISTANBUL: Lazy<Precompiles> = Lazy::new(|| {
        let mut precompiles = Precompiles::istanbul().clone();
        precompiles.extend([tendermint::TENDERMINT_HEADER_VALIDATION, iavl::IAVL_PROOF_VALIDATION]);
        precompiles
    });
    &ISTANBUL
}

/// Returns precompiles for Berlin spec.
pub fn berlin() -> &'static Precompiles {
    static BERLIN: Lazy<Precompiles> = Lazy::new(|| {
        let mut precompiles = Precompiles::berlin().clone();
        precompiles.extend([tendermint::TENDERMINT_HEADER_VALIDATION, iavl::IAVL_PROOF_VALIDATION]);
        precompiles
    });
    &BERLIN
}


/// Returns precompiles for Nano sepc.
pub fn nano() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = istanbul().clone();
        precompiles.extend([
            tendermint::TENDERMINT_HEADER_VALIDATION_NANO,
            iavl::IAVL_PROOF_VALIDATION_NANO,
        ]);

        Box::new(precompiles)
    })
}

/// Returns precompiles for Moran sepc.
pub fn moran() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = istanbul().clone();
        precompiles.extend([
            tendermint::TENDERMINT_HEADER_VALIDATION,
            iavl::IAVL_PROOF_VALIDATION_MORAN,
        ]);

        Box::new(precompiles)
    })
}

/// Returns precompiles for Planck sepc.
pub fn planck() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = istanbul().clone();
        precompiles
            .extend([tendermint::TENDERMINT_HEADER_VALIDATION, iavl::IAVL_PROOF_VALIDATION_PLANCK]);

        Box::new(precompiles)
    })
}

/// Returns precompiles for Luban sepc.
pub fn luban() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = planck().clone();
        precompiles.extend([
            bls::BLS_SIGNATURE_VALIDATION,
            cometbft::COMETBFT_LIGHT_BLOCK_VALIDATION_BEFORE_HERTZ,
        ]);

        Box::new(precompiles)
    })
}

/// Returns precompiles for Plato sepc.
pub fn plato() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = luban().clone();
        precompiles.extend([iavl::IAVL_PROOF_VALIDATION_PLATO]);

        Box::new(precompiles)
    })
}



/// Returns precompiles for Hertz sepc.
pub fn hertz() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = berlin().clone();
        precompiles.extend([cometbft::COMETBFT_LIGHT_BLOCK_VALIDATION]);

        Box::new(precompiles)
    })
}

/// Returns precompiles for Feynman sepc.
pub fn feynman() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
            let mut precompiles = hertz().clone();
            precompiles.extend([double_sign::DOUBLE_SIGN_EVIDENCE_VALIDATION]);

          
            #[cfg(feature = "secp256k1")]
            precompiles.extend([tm_secp256k1::TM_SECP256K1_SIGNATURE_RECOVER]);

        Box::new(precompiles)
    })
}

/// Returns precompiles for Cancun spec.
///
/// If the `c-kzg` feature is not enabled KZG Point Evaluation precompile will not be included,
/// effectively making this the same as Berlin.
pub fn cancun() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
            let mut precompiles = feynman().clone();
            
            // EIP-4844: Shard Blob Transactions
            cfg_if! {
                if #[cfg(any(feature = "c-kzg", feature = "kzg-rs"))] {
                    let precompile = kzg_point_evaluation::POINT_EVALUATION.clone();
                } else {
                    let precompile = PrecompileWithAddress(u64_to_address(0x0A), |_,_| Err(PrecompileError::Fatal("c-kzg feature is not enabled".into())));
                }
            }

            precompiles.extend([
                precompile,
            ]);

            Box::new(precompiles)
        })
}

/// Returns precompiles for Prague spec.
pub fn prague() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let precompiles = cancun().clone();

        // Don't include BLS12-381 precompiles in no_std builds.
        #[cfg(feature = "blst")]
        let precompiles = {
            let mut precompiles = precompiles;
            precompiles.extend(bls12_381::precompiles());
            precompiles
        };

        Box::new(precompiles)
    })
}

/// Returns precompiles for Haber spec.
pub fn haber() -> &'static Precompiles {
    static INSTANCE: OnceBox<Precompiles> = OnceBox::new();
    INSTANCE.get_or_init(|| {
        let precompiles = cancun().clone();

        #[cfg(feature = "secp256r1")]
        let precompiles = {
            let mut precompiles = precompiles;
            precompiles.extend([secp256r1::P256VERIFY]);
            precompiles
        };

        Box::new(precompiles)
    })
}

/// Returns the precompiles for the latest spec.
pub fn latest() -> &'static Precompiles {
    haber()
}

impl<CTX> PrecompileProvider<CTX> for BscPrecompiles
where
    CTX: ContextTr<Cfg: Cfg<Spec = BscSpecId>>,
{
    type Output = InterpreterResult;

    #[inline]
    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) {
        *self = Self::new_with_spec(spec);
    }

    #[inline]
    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        bytes: &Bytes,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, String> {
        self.inner.run(context, address, bytes, gas_limit)
    }

    #[inline]
    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.inner.warm_addresses()
    }

    #[inline]
    fn contains(&self, address: &Address) -> bool {
        self.inner.contains(address)
    }
}

impl Default for BscPrecompiles {
    fn default() -> Self {
        Self::new_with_spec(BscSpecId::CANCUN)
    }
}
