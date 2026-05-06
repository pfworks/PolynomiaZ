/// Disk geometry and data layout for polar coordinate compression.

/// Candidate sector sizes for adaptive geometry selection.
pub const SECTOR_CANDIDATES: &[usize] = &[8, 16, 32, 64, 128, 256, 512];

#[derive(Debug, Clone, Copy)]
pub struct DiskGeometry {
    pub tracks: usize,
    pub sectors_per_track: usize,
}

impl DiskGeometry {
    pub fn capacity(&self) -> usize {
        self.tracks * self.sectors_per_track
    }
}

pub fn compute_geometry(data_len: usize, sectors_per_track: usize) -> DiskGeometry {
    if data_len == 0 {
        return DiskGeometry { tracks: 0, sectors_per_track };
    }
    let tracks = (data_len + sectors_per_track - 1) / sectors_per_track;
    DiskGeometry { tracks, sectors_per_track }
}

/// Lay out data onto tracks. Pads last track with zeros.
pub fn lay_out(data: &[u8], geom: &DiskGeometry) -> Vec<Vec<u8>> {
    let mut tracks = Vec::with_capacity(geom.tracks);
    for t in 0..geom.tracks {
        let start = t * geom.sectors_per_track;
        let end = std::cmp::min(start + geom.sectors_per_track, data.len());
        let mut track = Vec::with_capacity(geom.sectors_per_track);
        track.extend_from_slice(&data[start..end]);
        track.resize(geom.sectors_per_track, 0);
        tracks.push(track);
    }
    tracks
}
