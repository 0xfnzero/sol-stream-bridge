use super::SharedConfig;
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use yellowstone_grpc_proto::{prelude::*, prost::Message, tonic::Status};

pub type Routes = HashMap<String, Vec<(u64, String)>>;
pub struct Aggregate {
    pub request: SubscribeRequest,
    pub routes: Routes,
    pub accounts: usize,
}
fn invalid(message: &'static str) -> Status {
    Status::invalid_argument(message)
}
fn address(value: &str, bytes: usize) -> Result<(), Status> {
    if value.len() > 88 {
        return Err(invalid("invalid public key/signature"));
    }
    let mut buf = [0u8; 64];
    if bs58::decode(value).onto(&mut buf[..]).ok() != Some(bytes) {
        return Err(invalid("invalid public key/signature"));
    }
    Ok(())
}
fn addresses<'a>(values: impl Iterator<Item = &'a String>) -> Result<(), Status> {
    for key in values {
        address(key, 32)?;
    }
    Ok(())
}
pub fn filter_count(r: &SubscribeRequest) -> usize {
    r.accounts.len()
        + r.transactions.len()
        + r.transactions_status.len()
        + r.slots.len()
        + r.blocks.len()
        + r.blocks_meta.len()
        + r.entry.len()
}
pub fn account_count(r: &SubscribeRequest) -> usize {
    r.accounts
        .values()
        .map(|f| f.account.len() + f.owner.len())
        .sum::<usize>()
        + r.transactions
            .values()
            .chain(r.transactions_status.values())
            .map(|f| f.account_include.len() + f.account_exclude.len() + f.account_required.len())
            .sum::<usize>()
        + r.blocks
            .values()
            .map(|f| f.account_include.len())
            .sum::<usize>()
}
pub fn validate(r: &SubscribeRequest, cfg: &SharedConfig) -> Result<(), Status> {
    if r.encoded_len() > cfg.max_request_bytes
        || filter_count(r) > cfg.max_filters_per_client
        || account_count(r) > cfg.max_accounts_per_client
    {
        return Err(Status::resource_exhausted(
            "subscription exceeds per-client limits",
        ));
    }
    if r.commitment
        .is_some_and(|c| Some(c) != cfg.commitment_level().ok())
    {
        return Err(invalid(
            "commitment must match the shared upstream commitment",
        ));
    }
    if r.from_slot.is_some() || !r.accounts_data_slice.is_empty() {
        return Err(invalid(
            "replay and account data slices are unavailable in shared mode",
        ));
    }
    for name in r
        .accounts
        .keys()
        .chain(r.transactions.keys())
        .chain(r.transactions_status.keys())
        .chain(r.slots.keys())
        .chain(r.blocks.keys())
        .chain(r.blocks_meta.keys())
        .chain(r.entry.keys())
    {
        if name.len() > 64 {
            return Err(invalid("filter names must be at most 64 bytes"));
        }
    }
    if !cfg.allow_accounts && !r.accounts.is_empty() {
        return Err(Status::permission_denied(
            "account updates disabled for this user",
        ));
    }
    for f in r.accounts.values() {
        if f.cuckoo_accounts_filter.is_some() {
            return Err(invalid("cuckoo filters are unavailable in shared mode"));
        }
        if !cfg.allow_unfiltered && f.account.is_empty() && f.owner.is_empty() {
            return Err(Status::permission_denied(
                "account or owner required; full account subscriptions disabled",
            ));
        }
        addresses(f.account.iter().chain(f.owner.iter()))?;
        if f.filters.len() > cfg.max_data_filters {
            return Err(Status::resource_exhausted("too many account data filters"));
        }
        for filter in &f.filters {
            use subscribe_request_filter_accounts_filter::Filter;
            use subscribe_request_filter_accounts_filter_memcmp::Data;
            match &filter.filter {
                Some(Filter::Memcmp(m)) => {
                    let bytes = match &m.data {
                        Some(Data::Bytes(b)) => b.clone(),
                        Some(Data::Base58(s)) if s.len() <= 180 => bs58::decode(s)
                            .into_vec()
                            .map_err(|_| invalid("invalid memcmp base58"))?,
                        Some(Data::Base64(s)) if s.len() <= 172 => STANDARD
                            .decode(s)
                            .map_err(|_| invalid("invalid memcmp base64"))?,
                        _ => return Err(invalid("invalid memcmp data")),
                    };
                    if bytes.len() > 128 || m.offset.checked_add(bytes.len() as u64).is_none() {
                        return Err(invalid("invalid memcmp size/offset"));
                    }
                }
                Some(Filter::Lamports(l)) if l.cmp.is_none() => {
                    return Err(invalid("lamports comparison required"))
                }
                Some(Filter::TokenAccountState(false)) | None => {
                    return Err(invalid("invalid account data filter"))
                }
                _ => {}
            }
        }
    }
    for f in r
        .transactions
        .values()
        .chain(r.transactions_status.values())
    {
        if f.cuckoo_account_include.is_some() {
            return Err(invalid("cuckoo filters are unavailable in shared mode"));
        }
        if !cfg.allow_unfiltered
            && f.account_include.is_empty()
            && f.account_required.is_empty()
            && f.signature.is_none()
        {
            return Err(Status::permission_denied("include/required account or signature required; full transaction subscriptions disabled"));
        }
        addresses(
            f.account_include
                .iter()
                .chain(f.account_exclude.iter())
                .chain(f.account_required.iter()),
        )?;
        if let Some(sig) = &f.signature {
            address(sig, 64)?;
        }
        if f.token_accounts.is_some_and(|v| !(0..=1).contains(&v)) {
            return Err(invalid("invalid token account expansion"));
        }
    }
    if !cfg.allow_blocks && !r.blocks.is_empty() {
        return Err(Status::permission_denied("block subscriptions disabled"));
    }
    if !cfg.allow_entries && !r.entry.is_empty() {
        return Err(Status::permission_denied("entry subscriptions disabled"));
    }
    for f in r.blocks.values() {
        if !cfg.allow_accounts && f.include_accounts == Some(true) {
            return Err(Status::permission_denied(
                "account updates disabled for this user",
            ));
        }
        if !cfg.allow_entries && f.include_entries == Some(true) {
            return Err(Status::permission_denied("entry subscriptions disabled"));
        }
        if f.cuckoo_account_include.is_some() {
            return Err(invalid("cuckoo filters are unavailable in shared mode"));
        }
        if !cfg.allow_unfiltered
            && (f.account_include.is_empty() || f.include_entries == Some(true))
        {
            return Err(Status::permission_denied("unfiltered block data disabled"));
        }
        addresses(f.account_include.iter())?;
    }
    Ok(())
}
fn merge<T: Message + Clone>(
    kind: &str,
    id: u64,
    src: &HashMap<String, T>,
    dst: &mut HashMap<String, T>,
    routes: &mut Routes,
) {
    for (name, filter) in src {
        // Content-addressed names never get reused for different filters. Old in-flight
        // updates therefore cannot be relabelled as a newly requested account/program.
        let label = format!("sb_{kind}_{:x}", Sha256::digest(filter.encode_to_vec()));
        dst.entry(label.clone()).or_insert_with(|| filter.clone());
        routes.entry(label).or_default().push((id, name.clone()));
    }
}
pub fn aggregate<'a>(
    clients: impl Iterator<Item = (u64, &'a SubscribeRequest)>,
    cfg: &SharedConfig,
) -> Result<Aggregate, Status> {
    let mut out = Aggregate {
        request: SubscribeRequest {
            commitment: Some(
                cfg.commitment_level()
                    .map_err(|_| invalid("invalid commitment"))?,
            ),
            ..Default::default()
        },
        routes: HashMap::new(),
        accounts: 0,
    };
    for (id, r) in clients {
        merge(
            "a",
            id,
            &r.accounts,
            &mut out.request.accounts,
            &mut out.routes,
        );
        merge(
            "t",
            id,
            &r.transactions,
            &mut out.request.transactions,
            &mut out.routes,
        );
        merge(
            "ts",
            id,
            &r.transactions_status,
            &mut out.request.transactions_status,
            &mut out.routes,
        );
        merge("s", id, &r.slots, &mut out.request.slots, &mut out.routes);
        merge("b", id, &r.blocks, &mut out.request.blocks, &mut out.routes);
        merge(
            "m",
            id,
            &r.blocks_meta,
            &mut out.request.blocks_meta,
            &mut out.routes,
        );
        merge("e", id, &r.entry, &mut out.request.entry, &mut out.routes);
    }
    out.accounts = account_count(&out.request);
    if filter_count(&out.request) > cfg.max_filters
        || out.accounts > cfg.max_accounts
        || out.request.encoded_len() > cfg.max_request_bytes
    {
        return Err(Status::resource_exhausted(
            "shared upstream filter/account limit reached",
        ));
    }
    Ok(out)
}
