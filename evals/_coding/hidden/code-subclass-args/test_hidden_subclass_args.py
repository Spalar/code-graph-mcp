"""Hidden grader for code-subclass-args: every graph class, not only Graph,
must accept a subclass with extra constructor arguments (networkx 75bdd73
changed all four; its own test covers only nx.Graph)."""

import pytest

import networkx as nx


@pytest.mark.parametrize("base", [nx.Graph, nx.DiGraph, nx.MultiGraph, nx.MultiDiGraph])
def test_subclass_extra_args(base):
    class MyGraph(base):
        def __init__(self, incoming_graph_data=None, extra_arg=None, **attr):
            super().__init__(incoming_graph_data, **attr)
            self.extra_arg = extra_arg

    G = MyGraph(extra_arg="extra arg")
    assert G.extra_arg == "extra arg"
    G = MyGraph([], "extra arg")
    assert G.extra_arg == "extra arg"
    G = MyGraph([(0, 1)], extra_arg="foo", name="bar")
    assert G.extra_arg == "foo"
    assert G.graph["name"] == "bar"
    assert sorted(G.edges()) == [(0, 1)]
    assert isinstance(G, base)
