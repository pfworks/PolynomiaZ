"""
Platter: maps raw bytes onto a disk geometry (tracks × sectors in polar coordinates).
"""

import math
from dataclasses import dataclass
from typing import List, Tuple


@dataclass
class DiskGeometry:
    tracks: int
    sectors_per_track: int

    @property
    def capacity(self) -> int:
        return self.tracks * self.sectors_per_track


def compute_geometry(data_len: int, sectors_per_track: int = 256) -> DiskGeometry:
    """Compute disk geometry to fit data_len bytes."""
    if data_len == 0:
        return DiskGeometry(tracks=0, sectors_per_track=sectors_per_track)
    tracks = math.ceil(data_len / sectors_per_track)
    return DiskGeometry(tracks=tracks, sectors_per_track=sectors_per_track)


def lay_out(data: bytes, geom: DiskGeometry) -> List[List[int]]:
    """Lay out data onto tracks. Pads last track with zeros if needed."""
    tracks = []
    for t in range(geom.tracks):
        start = t * geom.sectors_per_track
        end = start + geom.sectors_per_track
        chunk = data[start:end]
        track = list(chunk) + [0] * (geom.sectors_per_track - len(chunk))
        tracks.append(track)
    return tracks


def get_polar(track_idx: int, sector_idx: int, geom: DiskGeometry) -> Tuple[int, float]:
    """Return (r, θ) for a given track and sector."""
    r = track_idx
    theta = 2 * math.pi * sector_idx / geom.sectors_per_track
    return r, theta


# Candidate sector sizes for adaptive geometry selection
SECTOR_CANDIDATES = [8, 16, 32, 64, 128, 256, 512]
