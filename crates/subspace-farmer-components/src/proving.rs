use crate::auditing::ChunkCandidate;
use crate::reading::{read_record_metadata, read_sector_record_chunks, ReadingError};
use crate::sector::{
    SectorContentsMap, SectorContentsMapFromBytesError, SectorMetadataChecksummed,
};
use crate::{ReadAt, ReadAtSync};
use futures::FutureExt;
use std::collections::VecDeque;
use std::io;
use subspace_core_primitives::crypto::kzg::Kzg;
use subspace_core_primitives::{
    ChunkWitness, PieceOffset, PosSeed, PublicKey, Record, SBucket, SectorId, Solution,
    SolutionRange,
};
use subspace_erasure_coding::ErasureCoding;
use subspace_proof_of_space::Table;
use thiserror::Error;

/// Solutions that can be proven if necessary.
pub trait ProvableSolutions: ExactSizeIterator {
    /// Best solution distance found, `None` in case there are no solutions
    fn best_solution_distance(&self) -> Option<SolutionRange>;
}

/// Errors that happen during proving
#[derive(Debug, Error)]
pub enum ProvingError {
    /// Invalid erasure coding instance
    #[error("Invalid erasure coding instance")]
    InvalidErasureCodingInstance,
    /// Failed to create polynomial for record
    #[error("Failed to create polynomial for record at offset {piece_offset}: {error}")]
    FailedToCreatePolynomialForRecord {
        /// Piece offset
        piece_offset: PieceOffset,
        /// Lower-level error
        error: String,
    },
    /// Failed to create chunk witness
    #[error(
        "Failed to create chunk witness for record at offset {piece_offset} chunk {chunk_offset}: \
        {error}"
    )]
    FailedToCreateChunkWitness {
        /// Piece offset
        piece_offset: PieceOffset,
        /// Chunk index
        chunk_offset: u32,
        /// Lower-level error
        error: String,
    },
    /// Failed to decode sector contents map
    #[error("Failed to decode sector contents map: {0}")]
    FailedToDecodeSectorContentsMap(#[from] SectorContentsMapFromBytesError),
    /// I/O error occurred
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Record reading error
    #[error("Record reading error: {0}")]
    RecordReadingError(#[from] ReadingError),
}

#[derive(Debug, Clone)]
struct WinningChunk {
    /// Chunk offset within s-bucket
    chunk_offset: u32,
    /// Piece offset in a sector
    piece_offset: PieceOffset,
    /// Solution distance of this chunk
    solution_distance: SolutionRange,
}

/// Container for solution candidates.
#[derive(Debug)]
pub struct SolutionCandidates<'a, Sector>
where
    Sector: 'a,
{
    public_key: &'a PublicKey,
    sector_id: SectorId,
    s_bucket: SBucket,
    sector: Sector,
    sector_metadata: &'a SectorMetadataChecksummed,
    chunk_candidates: VecDeque<ChunkCandidate>,
}

impl<'a, Sector> Clone for SolutionCandidates<'a, Sector>
where
    Sector: Clone + 'a,
{
    fn clone(&self) -> Self {
        Self {
            public_key: self.public_key,
            sector_id: self.sector_id,
            s_bucket: self.s_bucket,
            sector: self.sector.clone(),
            sector_metadata: self.sector_metadata,
            chunk_candidates: self.chunk_candidates.clone(),
        }
    }
}

impl<'a, Sector> SolutionCandidates<'a, Sector>
where
    Sector: ReadAtSync + 'a,
{
    pub(crate) fn new(
        public_key: &'a PublicKey,
        sector_id: SectorId,
        s_bucket: SBucket,
        sector: Sector,
        sector_metadata: &'a SectorMetadataChecksummed,
        chunk_candidates: VecDeque<ChunkCandidate>,
    ) -> Self {
        Self {
            public_key,
            sector_id,
            s_bucket,
            sector,
            sector_metadata,
            chunk_candidates,
        }
    }

    /// Total number of candidates
    pub fn len(&self) -> usize {
        self.chunk_candidates.len()
    }

    /// Returns true if no candidates inside
    pub fn is_empty(&self) -> bool {
        self.chunk_candidates.is_empty()
    }

    /// Turn solution candidates into actual solutions
    pub fn into_solutions<RewardAddress, PosTable, TableGenerator>(
        self,
        reward_address: &'a RewardAddress,
        kzg: &'a Kzg,
        erasure_coding: &'a ErasureCoding,
        table_generator: TableGenerator,
    ) -> Result<impl ProvableSolutions<Item = MaybeSolution<RewardAddress>> + 'a, ProvingError>
    where
        RewardAddress: Copy,
        PosTable: Table,
        TableGenerator: (FnMut(&PosSeed) -> PosTable) + 'a,
    {
        SolutionsIterator::<'a, _, PosTable, _, _>::new(
            self.public_key,
            reward_address,
            self.sector_id,
            self.s_bucket,
            self.sector,
            self.sector_metadata,
            kzg,
            erasure_coding,
            self.chunk_candidates,
            table_generator,
        )
    }
}

type MaybeSolution<RewardAddress> = Result<Solution<PublicKey, RewardAddress>, ProvingError>;

struct SolutionsIterator<'a, RewardAddress, PosTable, TableGenerator, Sector>
where
    Sector: ReadAtSync + 'a,
    PosTable: Table,
    TableGenerator: (FnMut(&PosSeed) -> PosTable) + 'a,
{
    public_key: &'a PublicKey,
    reward_address: &'a RewardAddress,
    sector_id: SectorId,
    s_bucket: SBucket,
    sector_metadata: &'a SectorMetadataChecksummed,
    s_bucket_offsets: Box<[u32; Record::NUM_S_BUCKETS]>,
    kzg: &'a Kzg,
    erasure_coding: &'a ErasureCoding,
    sector_contents_map: SectorContentsMap,
    sector: ReadAt<Sector, !>,
    winning_chunks: VecDeque<WinningChunk>,
    count: usize,
    best_solution_distance: Option<SolutionRange>,
    table_generator: TableGenerator,
}

impl<'a, RewardAddress, PosTable, TableGenerator, Sector> ExactSizeIterator
    for SolutionsIterator<'a, RewardAddress, PosTable, TableGenerator, Sector>
where
    RewardAddress: Copy,
    Sector: ReadAtSync + 'a,
    PosTable: Table,
    TableGenerator: (FnMut(&PosSeed) -> PosTable) + 'a,
{
}

impl<'a, RewardAddress, PosTable, TableGenerator, Sector> Iterator
    for SolutionsIterator<'a, RewardAddress, PosTable, TableGenerator, Sector>
where
    RewardAddress: Copy,
    Sector: ReadAtSync + 'a,
    PosTable: Table,
    TableGenerator: (FnMut(&PosSeed) -> PosTable) + 'a,
{
    type Item = MaybeSolution<RewardAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let WinningChunk {
            chunk_offset,
            piece_offset,
            solution_distance: _,
        } = self.winning_chunks.pop_front()?;

        self.count -= 1;

        // Derive PoSpace table
        let pos_table = (self.table_generator)(
            &self
                .sector_id
                .derive_evaluation_seed(piece_offset, self.sector_metadata.history_size),
        );

        let maybe_solution: Result<_, ProvingError> = try {
            let sector_record_chunks_fut = read_sector_record_chunks(
                piece_offset,
                self.sector_metadata.pieces_in_sector,
                &self.s_bucket_offsets,
                &self.sector_contents_map,
                &pos_table,
                &self.sector,
            );
            let sector_record_chunks = sector_record_chunks_fut
                .now_or_never()
                .expect("Sync reader; qed")?;

            let chunk = sector_record_chunks
                .get(usize::from(self.s_bucket))
                .expect("Within s-bucket range; qed")
                .expect("Winning chunk was plotted; qed");

            let source_chunks_polynomial = self
                .erasure_coding
                .recover_poly(sector_record_chunks.as_slice())
                .map_err(|error| ReadingError::FailedToErasureDecodeRecord {
                    piece_offset,
                    error,
                })?;
            drop(sector_record_chunks);

            // NOTE: We do not check plot consistency using checksum because it is more
            // expensive and consensus will verify validity of the proof anyway
            let record_metadata_fut = read_record_metadata(
                piece_offset,
                self.sector_metadata.pieces_in_sector,
                &self.sector,
            );
            let record_metadata = record_metadata_fut
                .now_or_never()
                .expect("Sync reader; qed")?;

            let proof_of_space = pos_table.find_proof(self.s_bucket.into()).expect(
                "Quality exists for this s-bucket, otherwise it wouldn't be a winning chunk; qed",
            );

            let chunk_witness = self
                .kzg
                .create_witness(
                    &source_chunks_polynomial,
                    Record::NUM_S_BUCKETS,
                    self.s_bucket.into(),
                )
                .map_err(|error| ProvingError::FailedToCreateChunkWitness {
                    piece_offset,
                    chunk_offset,
                    error,
                })?;

            Solution {
                public_key: *self.public_key,
                reward_address: *self.reward_address,
                sector_index: self.sector_metadata.sector_index,
                history_size: self.sector_metadata.history_size,
                piece_offset,
                record_commitment: record_metadata.commitment,
                record_witness: record_metadata.witness,
                chunk,
                chunk_witness: ChunkWitness::from(chunk_witness),
                proof_of_space,
            }
        };

        match maybe_solution {
            Ok(solution) => Some(Ok(solution)),
            Err(error) => Some(Err(error)),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<'a, RewardAddress, PosTable, TableGenerator, Sector> ProvableSolutions
    for SolutionsIterator<'a, RewardAddress, PosTable, TableGenerator, Sector>
where
    RewardAddress: Copy,
    Sector: ReadAtSync + 'a,
    PosTable: Table,
    TableGenerator: (FnMut(&PosSeed) -> PosTable) + 'a,
{
    fn best_solution_distance(&self) -> Option<SolutionRange> {
        self.best_solution_distance
    }
}

impl<'a, RewardAddress, PosTable, TableGenerator, Sector>
    SolutionsIterator<'a, RewardAddress, PosTable, TableGenerator, Sector>
where
    RewardAddress: Copy,
    Sector: ReadAtSync + 'a,
    PosTable: Table,
    TableGenerator: (FnMut(&PosSeed) -> PosTable) + 'a,
{
    #[allow(clippy::too_many_arguments)]
    fn new(
        public_key: &'a PublicKey,
        reward_address: &'a RewardAddress,
        sector_id: SectorId,
        s_bucket: SBucket,
        sector: Sector,
        sector_metadata: &'a SectorMetadataChecksummed,
        kzg: &'a Kzg,
        erasure_coding: &'a ErasureCoding,
        chunk_candidates: VecDeque<ChunkCandidate>,
        table_generator: TableGenerator,
    ) -> Result<Self, ProvingError> {
        if erasure_coding.max_shards() < Record::NUM_S_BUCKETS {
            return Err(ProvingError::InvalidErasureCodingInstance);
        }

        let sector_contents_map = {
            let mut sector_contents_map_bytes =
                vec![0; SectorContentsMap::encoded_size(sector_metadata.pieces_in_sector)];

            sector.read_at(&mut sector_contents_map_bytes, 0)?;

            SectorContentsMap::from_bytes(
                &sector_contents_map_bytes,
                sector_metadata.pieces_in_sector,
            )?
        };

        let s_bucket_records = sector_contents_map
            .iter_s_bucket_records(s_bucket)
            .expect("S-bucket audit index is guaranteed to be in range; qed")
            .collect::<Vec<_>>();
        let winning_chunks = chunk_candidates
            .into_iter()
            .filter_map(move |chunk_candidate| {
                let (piece_offset, encoded_chunk_used) = s_bucket_records
                    .get(chunk_candidate.chunk_offset as usize)
                    .expect("Wouldn't be a candidate if wasn't within s-bucket; qed");

                encoded_chunk_used.then_some(WinningChunk {
                    chunk_offset: chunk_candidate.chunk_offset,
                    piece_offset: *piece_offset,
                    solution_distance: chunk_candidate.solution_distance,
                })
            })
            .collect::<VecDeque<_>>();

        let best_solution_distance = winning_chunks
            .front()
            .map(|winning_chunk| winning_chunk.solution_distance);

        let s_bucket_offsets = sector_metadata.s_bucket_offsets();

        let count = winning_chunks.len();

        Ok(Self {
            public_key,
            reward_address,
            sector_id,
            s_bucket,
            sector_metadata,
            s_bucket_offsets,
            kzg,
            erasure_coding,
            sector_contents_map,
            sector: ReadAt::from_sync(sector),
            winning_chunks,
            count,
            best_solution_distance,
            table_generator,
        })
    }
}
