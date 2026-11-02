import math

import pytest

from fastcheck import crossing, nearest


def test_crossing_at_the_intersection() -> None:
    assert crossing((0, 0), (2, 2), (0, 2), (2, 0)) == pytest.approx((1, 1))


def test_crossing_off_center() -> None:
    assert crossing((0, 0), (4, 0), (1, -1), (1, 3)) == pytest.approx((1, 0))


def test_parallel_segments_do_not_cross() -> None:
    assert crossing((0, 0), (2, 0), (0, 1), (2, 1)) is None


def test_collinear_segments_do_not_cross() -> None:
    assert crossing((0, 0), (2, 0), (1, 0), (3, 0)) is None


def test_touching_at_an_end_is_not_a_crossing() -> None:
    assert crossing((0, 0), (2, 0), (2, 0), (2, 2)) is None


def test_segments_that_would_cross_if_extended_do_not() -> None:
    assert crossing((0, 0), (1, 0), (2, -1), (2, 1)) is None


def test_nearest_sorted_and_limited() -> None:
    assert nearest((0, 0), [(3, 4), (1, 0), (0, 2)], 2) == [1, 2]


def test_nearest_fewer_points_than_asked() -> None:
    assert nearest((0, 0), [(1, 1)], 2) == [math.sqrt(2)]


def test_nearest_no_points() -> None:
    assert nearest((0, 0), [], 1) == []
