use super::{
    bench::{bench, BenchKind},
    Command,
};
use rand::{seq::SliceRandom, Rng};
use reth_db::{static_file::HeaderMask, DatabaseEnv};
use reth_primitives::{
    static_file::{Compression, Filters, InclusionFilter, PerfectHashingFunction},
    BlockHash, Header, StaticFileSegment,
};
use reth_provider::{
    providers::StaticFileProvider, BlockNumReader, HeaderProvider, ProviderError, ProviderFactory,
};
use std::{ops::RangeInclusive, path::PathBuf, sync::Arc};

impl Command {
    pub(crate) fn bench_headers_static_file(
        &self,
        provider_factory: Arc<ProviderFactory<DatabaseEnv>>,
        compression: Compression,
        inclusion_filter: InclusionFilter,
        phf: Option<PerfectHashingFunction>,
    ) -> eyre::Result<()> {
        let provider = provider_factory.provider()?;
        let tip = provider.last_block_number()?;
        let block_range = *self.block_ranges(tip).first().expect("has been generated before");

        let filters = if let Some(phf) = self.with_filters.then_some(phf).flatten() {
            Filters::WithFilters(inclusion_filter, phf)
        } else {
            Filters::WithoutFilters
        };

        let range: RangeInclusive<u64> = (&block_range).into();
        let mut row_indexes = range.collect::<Vec<_>>();
        let mut rng = rand::thread_rng();

        let path: PathBuf = StaticFileSegment::Headers
            .filename_with_configuration(filters, compression, &block_range)
            .into();
        let provider = StaticFileProvider::new(PathBuf::default())?;
        let jar_provider = provider.get_segment_provider_from_block(
            StaticFileSegment::Headers,
            self.from,
            Some(&path),
        )?;
        let mut cursor = jar_provider.cursor()?;

        for bench_kind in [BenchKind::Walk, BenchKind::RandomAll] {
            bench(
                bench_kind,
                provider_factory.clone(),
                StaticFileSegment::Headers,
                filters,
                compression,
                || {
                    for num in row_indexes.iter() {
                        cursor
                            .get_one::<HeaderMask<Header>>((*num).into())?
                            .ok_or(ProviderError::HeaderNotFound((*num).into()))?;
                    }
                    Ok(())
                },
                |provider| {
                    for num in row_indexes.iter() {
                        provider
                            .header_by_number(*num)?
                            .ok_or(ProviderError::HeaderNotFound((*num).into()))?;
                    }
                    Ok(())
                },
            )?;

            // For random walk
            row_indexes.shuffle(&mut rng);
        }

        // BENCHMARK QUERYING A RANDOM HEADER BY NUMBER
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())];
            bench(
                BenchKind::RandomOne,
                provider_factory.clone(),
                StaticFileSegment::Headers,
                filters,
                compression,
                || {
                    Ok(cursor
                        .get_one::<HeaderMask<Header>>(num.into())?
                        .ok_or(ProviderError::HeaderNotFound(num.into()))?)
                },
                |provider| {
                    Ok(provider
                        .header_by_number(num)?
                        .ok_or(ProviderError::HeaderNotFound((num).into()))?)
                },
            )?;
        }

        // BENCHMARK QUERYING A RANDOM HEADER BY HASH
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())];
            let header_hash = provider_factory
                .header_by_number(num)?
                .ok_or(ProviderError::HeaderNotFound(num.into()))?
                .hash_slow();

            bench(
                BenchKind::RandomHash,
                provider_factory,
                StaticFileSegment::Headers,
                filters,
                compression,
                || {
                    let (header, hash) = cursor
                        .get_two::<HeaderMask<Header, BlockHash>>((&header_hash).into())?
                        .ok_or(ProviderError::HeaderNotFound(header_hash.into()))?;

                    // Might be a false positive, so in the real world we have to validate it
                    assert_eq!(hash, header_hash);

                    Ok(header)
                },
                |provider| {
                    Ok(provider
                        .header(&header_hash)?
                        .ok_or(ProviderError::HeaderNotFound(header_hash.into()))?)
                },
            )?;
        }
        Ok(())
    }
}
