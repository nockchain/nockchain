use argon2::{Algorithm, Argon2, AssociatedData, Params, Version};
use nockapp::utils::bytes::Byts;
use nockapp::utils::make_tas;
use nockapp::{AtomExt, Noun};
use nockvm::jets::cold::{Nounable, NounableResult};
use nockvm::noun::{NounAllocator, Slots, D, T};

#[derive(Debug, Clone)]
pub struct Argon2Args {
    pub out: usize,
    pub secret: Byts,
    pub params: Params,
    pub algorithm: Algorithm,
    pub version: Version,
}

impl Argon2Args {
    pub fn new(
        out: usize,
        secret: Vec<u8>,
        params: Params,
        algorithm: Algorithm,
        version: Version,
    ) -> Self {
        Self {
            out,
            secret: Byts(secret),
            params,
            algorithm,
            version,
        }
    }
}

impl Nounable for Argon2Args {
    type Target = Self;
    fn into_noun<A: NounAllocator>(self, stack: &mut A) -> Noun {
        let out = D(self.out as u64);
        let secret = self.secret.into_noun(stack);
        let threads = D(self.params.p_cost() as u64);
        let mem_cost = D(self.params.m_cost() as u64);
        let time_cost = D(self.params.t_cost() as u64);
        let extra_byts = Byts(self.params.data().to_vec());
        let extra = extra_byts.into_noun(stack);
        let typ = match self.algorithm {
            Algorithm::Argon2d => "d",
            Algorithm::Argon2i => "i",
            Algorithm::Argon2id => "id",
        };
        let vers = match self.version {
            Version::V0x10 => D(0x10),
            Version::V0x13 => D(0x13),
        };
        let typ_noun = make_tas(stack, typ).as_noun();
        T(
            stack,
            &[out, typ_noun, vers, threads, mem_cost, time_cost, secret, extra],
        )
    }
    fn from_noun<A: NounAllocator>(stack: &mut A, params: &Noun) -> NounableResult<Self::Target> {
        let out = params.slot(2)?.as_atom()?.as_u64()? as usize;
        let typ = params
            .slot(6)?
            .as_atom()?
            .into_string()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let version = params.slot(14)?.as_atom()?.as_u64()? as u8;
        let threads = params.slot(30)?.as_atom()?.as_u64()? as u32;
        let mem_cost = params.slot(62)?.as_atom()?.as_u64()? as u32;
        let time_cost = params.slot(126)?.as_atom()?.as_u64()? as u32;
        let secret = Byts::from_noun(stack, &params.slot(254)?)?;
        let extra = Byts::from_noun(stack, &params.slot(255)?)?;

        // prepare parameters
        let data = AssociatedData::new(&extra.0).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

        // translate threads, mem_cost, time_cost, and extra into Argon2 params
        let params = argon2::ParamsBuilder::new()
            .p_cost(threads)
            .m_cost(mem_cost)
            .t_cost(time_cost)
            .data(data)
            .build()
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });

        let algorithm = match typ.as_str() {
            "d" => argon2::Algorithm::Argon2d,
            "i" => argon2::Algorithm::Argon2i,
            "id" => argon2::Algorithm::Argon2id,
            _ => {
                return Err(nockvm::noun::Error::NotRepresentable)?;
            }
        };
        let version = match version {
            0x10 => argon2::Version::V0x10,
            0x13 => argon2::Version::V0x13,
            _ => {
                return Err(nockvm::noun::Error::NotRepresentable)?;
            }
        };

        Ok(Argon2Args {
            out,
            secret,
            params,
            algorithm,
            version,
        })
    }
}

pub fn argon2_hook(
    args: Argon2Args,
    password: &[u8],
    salt: &[u8],
    res: &mut [u8],
) -> Result<(), argon2::Error> {
    let secret = args.secret.0;
    let algorithm = args.algorithm;
    let version = args.version;
    let params = args.params;

    let ctx = Argon2::new_with_secret(&secret, algorithm, version, params.clone()).unwrap_or_else(
        |err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        },
    );
    ctx.hash_password_into(password, salt, res)
}
