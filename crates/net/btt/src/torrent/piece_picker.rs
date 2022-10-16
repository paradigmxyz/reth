use crate::{bitfield::BitField, info::PieceIndex};
use tracing::trace;

pub(crate) struct PiecePicker {
    /// Represents the pieces that we have downloaded.
    ///
    /// The bitfield is pre-allocated to the number of pieces in the torrent and
    /// each field that we have is set to true.
    own_pieces: BitField,
    /// We collect metadata about pieces in the torrent swarm in this vector.
    ///
    /// The vector is pre-allocated to the number of pieces in the torrent.
    pieces: Vec<Piece>,
    /// A cache for the number of pieces we haven't received yet (but may have
    /// picked).
    missing_count: usize,
    /// A cache for the number of pieces that can be picked.
    free_count: usize,
}

/// Metadata about a piece relevant for the piece picker.
#[derive(Clone, Copy, Default)]
pub(crate) struct Piece {
    /// The frequency of this piece in the torrent swarm.
    pub frequency: usize,
    /// Whether we have already picked this piece and are currently downloading
    /// it. This flag is set to true when the piece is picked.
    ///
    /// This is to prevent picking the same piece we are already downloading in
    /// the scenario in which we want to pick a new piece before the already
    /// downloadng piece finishes. Not having this check would lead us to always
    /// pick this piece until we tell the piece picker that we have it and thus
    /// wouldn't be able to download multiple pieces simultaneously (an
    /// important optimizaiton step).
    pub is_pending: bool,
}

impl PiecePicker {
    /// Creates a new piece picker with the given own_pieces we already have.
    pub fn new(own_pieces: BitField) -> Self {
        let mut pieces = Vec::new();
        pieces.resize_with(own_pieces.len(), Piece::default);
        let missing_count = own_pieces.count_zeros();
        Self { own_pieces, pieces, missing_count, free_count: missing_count }
    }

    /// Returns an immutable reference to a bitfield of the pieces we own.
    pub fn own_pieces(&self) -> &BitField {
        &self.own_pieces
    }

    /// Returns the number of missing pieces that are needed to complete the
    /// download.
    pub fn missing_piece_count(&self) -> usize {
        self.missing_count
    }

    /// Returns true if all pieces have been picked (whether pending or
    /// recieved).
    pub fn all_pieces_picked(&self) -> bool {
        self.free_count == 0
    }

    /// Returns the first piece that we don't yet have and isn't already being
    /// downloaded, or None, if no piece can be picked at this time.
    pub fn pick_piece(&mut self) -> Option<PieceIndex> {
        trace!("Picking next piece");

        for index in 0..self.own_pieces.len() {
            // only consider this piece if we don't have it and if we are not
            // already downloading it (whether it's not pending)
            debug_assert!(index < self.pieces.len());
            let piece = &mut self.pieces[index];
            if !self.own_pieces[index] && piece.frequency > 0 && !piece.is_pending {
                // set pending flag on piece so that this piece is not picked
                // again (see note on field)
                piece.is_pending = true;
                self.free_count -= 1;
                trace!("Picked piece {}", index);
                return Some(index)
            }
        }

        // no piece could be picked
        trace!("Could not pick piece");
        None
    }

    /// Registers the avilability of a peer's pieces and returns whether we're
    /// interested in peer's pieces.
    ///
    /// # Panics
    ///
    /// Panics if the peer sent us pieces with a different count than ours.
    /// The validity of the pieces must be ensured at the protocol level (in
    /// [`crate::peer::PeerSession`]).
    pub fn register_peer_pieces(&mut self, pieces: &BitField) -> bool {
        trace!("Registering piece availability: {}", pieces);

        assert_eq!(
            pieces.len(),
            self.own_pieces.len(),
            "peer's bitfield must be the same length as ours"
        );

        let mut interested = false;
        for (index, (have_piece, peer_has_piece)) in
            self.own_pieces.iter().zip(pieces.iter()).enumerate()
        {
            // increase frequency count for this piece if peer has it
            if *peer_has_piece {
                self.pieces[index].frequency += 1;
                // if we don't have at least one piece peer has, we're
                // interested
                if !have_piece {
                    interested = true;
                }
            }
        }

        interested
    }

    /// Increments the availability of a piece.
    ///
    /// This should be called when a peer sends us a `have` message of a new
    /// piece.
    ///
    /// # Panics
    ///
    /// Panics if the piece index is out of range. The index validity must be
    /// ensured at the protocol level (in [`crate::peer::PeerSession`]).
    pub fn register_peer_piece(&mut self, index: PieceIndex) -> bool {
        trace!("Registering newly available piece {}", index);
        let is_interested = self.own_pieces.get(index).expect("invalid piece index");
        self.pieces[index].frequency += 1;
        *is_interested
    }

    /// Tells the piece picker that we have downloaded the piece at the given
    /// index that we had picked before.
    ///
    /// # Panics
    ///
    /// Panics if the piece was already received.
    pub fn received_piece(&mut self, index: PieceIndex) {
        trace!("Registering received piece {}", index);

        // we assert here as this method is only called by internal methods on
        // piece completion, meaning the piece must exist (we can't download an
        // invalid piece)
        let mut have_piece = self.own_pieces.get_mut(index).expect("invalid piece index");
        // we must not already have this piece as otherwise the free/missing
        // count logic is thrown off
        assert!(!*have_piece);

        // register owned piece
        *have_piece = true;
        self.missing_count -= 1;

        // This is an edge-case and shouldn't normally happen, but we guard
        // against it anyway in case there are changes in other parts of the
        // code.
        // If the piece was received without it having previously been picked,
        // we need to decrease the free piece count here, as it is normally done
        // in the `pick_piece` method.
        let piece = &mut self.pieces[index];
        if !piece.is_pending {
            self.free_count -= 1;
            // also set that this piece is no longer pending (even though we
            // won't be downloading it anymore, later we may re-download a piece
            // in which case not resetting the flag would cause us to never pick
            // the piece again)
            piece.is_pending = false;
        }
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Tests that repeatedly requesting as many pieces as are in the piece
    /// picker returns all pieces, none of them previously picked.
    #[test]
    fn should_pick_all_pieces() {
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);
        let available_pieces = BitField::new_all_set(piece_count);
        piece_picker.register_peer_pieces(&available_pieces);

        // save picked pieces
        let mut picked = HashSet::with_capacity(piece_count);

        // pick all pieces one by one
        for index in 0..piece_count {
            let pick = piece_picker.pick_piece();
            // for now we assert that we pick pieces in sequential order, but
            // later, when we add different algorithms, this line has to change
            assert_eq!(pick, Some(index));
            let pick = pick.unwrap();
            // assert that this piece hasn't been picked before
            assert!(!picked.contains(&pick));
            // mark piece as picked
            picked.insert(pick);
        }

        // assert that we picked all pieces
        assert_eq!(picked.len(), piece_count);
    }

    /// Tests registering a received piece causes the piece picker to not pick
    /// that piece again.
    #[test]
    fn should_mark_piece_as_received() {
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);
        let available_pieces = BitField::new_all_set(piece_count);
        piece_picker.register_peer_pieces(&available_pieces);
        assert!(piece_picker.own_pieces.not_any());

        // mark pieces as received
        let owned_pieces = [3, 10, 5];
        for index in owned_pieces.iter() {
            piece_picker.received_piece(*index);
            assert!(piece_picker.own_pieces[*index]);
        }
        assert!(!piece_picker.own_pieces.is_empty());

        // request pieces to pick next and make sure the ones we already have
        // are not picked
        for _ in 0..piece_count - owned_pieces.len() {
            let pick = piece_picker.pick_piece().unwrap();
            // assert that it's not a piece we already have
            assert!(owned_pieces.iter().all(|owned| *owned != pick));
        }
    }

    #[test]
    fn should_count_missing_pieces() {
        // empty piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);

        assert_eq!(piece_picker.missing_piece_count(), piece_count);

        // set 2 pieces
        let have_count = 2;
        for index in 0..have_count {
            piece_picker.received_piece(index);
        }
        assert_eq!(piece_picker.missing_piece_count(), piece_count - have_count);

        // set all pieces
        for index in have_count..piece_count {
            piece_picker.received_piece(index);
        }
        assert_eq!(piece_picker.missing_piece_count(), 0);
    }

    /// Tests that the piece picker correctly reports pieces that were not
    /// picked or received.
    #[test]
    fn should_count_free_pieces() {
        // empty piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);
        // NOTE: need to register frequency before we pick any pieces
        piece_picker.register_peer_pieces(&BitField::new_all_set(piece_count));

        assert_eq!(piece_picker.free_count, piece_count);

        // picked and received 2 pieces
        for i in 0..2 {
            assert!(piece_picker.pick_piece().is_some());
            piece_picker.received_piece(i);
        }
        assert_eq!(piece_picker.free_count, 13);

        // pick 3 pieces
        for _ in 0..3 {
            assert!(piece_picker.pick_piece().is_some());
        }
        assert_eq!(piece_picker.free_count, 10);

        // received 1 of the above picked pieces: shouldn't change outcome
        piece_picker.received_piece(2);
        assert_eq!(piece_picker.free_count, 10);

        // pick rest of the pieces
        for _ in 0..10 {
            assert!(piece_picker.pick_piece().is_some());
        }
        assert!(piece_picker.all_pieces_picked());
    }

    /// Tests that the piece picker correctly determines whether we are
    /// interested in a variety of piece sets.
    // TODO: break this up into smaller tests
    #[test]
    fn should_determine_interest() {
        // empty piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);

        // we are interested if peer has all pieces
        let available_pieces = BitField::new_all_set(piece_count);
        assert!(piece_picker.register_peer_pieces(&available_pieces));

        // we are also interested if peer has at least a single piece
        let mut available_pieces = BitField::new_all_clear(piece_count);
        available_pieces.set(0, true);
        assert!(piece_picker.register_peer_pieces(&available_pieces));

        // half full piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);
        for index in 0..8 {
            piece_picker.received_piece(index);
        }

        // we are not interested in peer that has the same pieces we do
        let mut available_pieces = BitField::new_all_clear(piece_count);
        for index in 0..8 {
            available_pieces.set(index, true);
        }
        assert!(!piece_picker.register_peer_pieces(&available_pieces));

        // we are interested in peer that has at least a single piece we don't
        let mut available_pieces = BitField::new_all_clear(piece_count);
        for index in 0..9 {
            available_pieces.set(index, true);
        }
        assert!(piece_picker.register_peer_pieces(&available_pieces));

        // full piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::empty(piece_count);
        for index in 0..piece_count {
            piece_picker.received_piece(index);
        }

        // we are not interested in any pieces since we own all of them
        assert!(!piece_picker.register_peer_pieces(&available_pieces));
    }

    impl PiecePicker {
        fn empty(piece_count: usize) -> Self {
            Self::new(BitField::new_all_clear(piece_count))
        }
    }
}
