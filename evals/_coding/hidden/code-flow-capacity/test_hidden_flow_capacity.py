"""Hidden grader for code-flow-capacity: a callable `capacity(u, v, d)` must
work everywhere a capacity attribute name does, and give the same answers.

Only public entry points are exercised, so any correct implementation passes.
"""

import pytest

import networkx as nx
from networkx.algorithms import flow

FLOW_FUNCS = [
    flow.boykov_kolmogorov,
    flow.dinitz,
    flow.edmonds_karp,
    flow.preflow_push,
    flow.shortest_augmenting_path,
]


def _digraph():
    G = nx.DiGraph()
    for u, v, c in [
        ("s", "a", 3),
        ("s", "b", 2),
        ("a", "b", 1),
        ("a", "t", 2),
        ("b", "t", 3),
        ("b", "c", 4),
        ("c", "t", 1),
    ]:
        G.add_edge(u, v, capacity=c, cap10=10 * c)
    return G


def _by_attr(u, v, d):
    return d["capacity"]


def _times_ten(u, v, d):
    return d["capacity"] * 10


@pytest.mark.parametrize("flow_func", FLOW_FUNCS)
def test_flow_value_callable_matches_attribute(flow_func):
    G = _digraph()
    expected = nx.maximum_flow_value(G, "s", "t", flow_func=flow_func)
    got = nx.maximum_flow_value(G, "s", "t", capacity=_by_attr, flow_func=flow_func)
    assert got == expected
    scaled = nx.maximum_flow_value(
        G, "s", "t", capacity=_times_ten, flow_func=flow_func
    )
    assert scaled == 10 * expected
    assert scaled == nx.maximum_flow_value(
        G, "s", "t", capacity="cap10", flow_func=flow_func
    )


@pytest.mark.parametrize("flow_func", FLOW_FUNCS)
def test_maximum_flow_dict_respects_callable(flow_func):
    G = _digraph()
    value, flow_dict = nx.maximum_flow(
        G, "s", "t", capacity=_times_ten, flow_func=flow_func
    )
    assert value == 10 * nx.maximum_flow_value(G, "s", "t")
    for u, nbrs in flow_dict.items():
        for v, f in nbrs.items():
            assert 0 <= f <= 10 * G[u][v]["capacity"]


@pytest.mark.parametrize("flow_func", FLOW_FUNCS)
def test_minimum_cut_callable(flow_func):
    G = _digraph()
    cut_value, (S, T) = nx.minimum_cut(
        G, "s", "t", capacity=_times_ten, flow_func=flow_func
    )
    assert cut_value == 10 * nx.minimum_cut_value(G, "s", "t")
    assert "s" in S and "t" in T
    crossing = sum(10 * G[u][v]["capacity"] for u in S for v in G[u] if v in T)
    assert crossing == cut_value
    assert (
        nx.minimum_cut_value(G, "s", "t", capacity=_times_ten, flow_func=flow_func)
        == cut_value
    )


@pytest.mark.parametrize("flow_func", FLOW_FUNCS)
def test_flow_func_called_directly_with_callable(flow_func):
    G = _digraph()
    R = flow_func(G, "s", "t", capacity=_times_ten)
    assert R.graph["flow_value"] == 10 * nx.maximum_flow_value(G, "s", "t")


@pytest.mark.parametrize("flow_func", FLOW_FUNCS)
def test_callable_infinite_capacity(flow_func):
    G = nx.DiGraph()
    G.add_edge("s", "a", capacity=5)
    G.add_edge("a", "t")  # no attribute: infinite with the string form
    expected = nx.maximum_flow_value(G, "s", "t", flow_func=flow_func)
    got = nx.maximum_flow_value(
        G,
        "s",
        "t",
        capacity=lambda u, v, d: d.get("capacity", float("inf")),
        flow_func=flow_func,
    )
    assert got == expected == 5


def test_undirected_callable():
    G = nx.Graph()
    for u, v, c in [(0, 1, 3), (1, 2, 2), (0, 2, 1), (2, 3, 4)]:
        G.add_edge(u, v, capacity=c)
    for flow_func in FLOW_FUNCS:
        assert nx.maximum_flow_value(
            G, 0, 3, capacity=_times_ten, flow_func=flow_func
        ) == 10 * nx.maximum_flow_value(G, 0, 3, flow_func=flow_func)


def test_gomory_hu_tree_callable():
    G = nx.karate_club_graph()
    for i, (u, v) in enumerate(G.edges()):
        G[u][v]["capacity"] = 1 + i % 5
    T_attr = nx.gomory_hu_tree(G)
    T_call = nx.gomory_hu_tree(G, capacity=lambda u, v, d: d["capacity"] * 10)
    # Same min-cut values between every pair, scaled by ten.
    for u, v in [(0, 33), (5, 16), (2, 30), (11, 20)]:
        path_a = nx.shortest_path(T_attr, u, v, weight="weight")
        path_c = nx.shortest_path(T_call, u, v, weight="weight")
        min_a = min(T_attr[a][b]["weight"] for a, b in nx.utils.pairwise(path_a))
        min_c = min(T_call[a][b]["weight"] for a, b in nx.utils.pairwise(path_c))
        assert min_c == 10 * min_a
