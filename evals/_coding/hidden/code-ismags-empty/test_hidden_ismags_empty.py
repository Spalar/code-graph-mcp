"""Hidden grader for code-ismags-empty: the regression tests upstream added
with the fix (networkx c94928e), minus the assertion on the internal
N_node_colors attribute, which a correct fix need not touch."""

import networkx as nx
from networkx.algorithms import isomorphism as iso


def test_largest_subgraph_null_graph_cases():
    graph = nx.path_graph(5)
    ismags = iso.ISMAGS(nx.Graph(), graph)
    assert list(ismags.largest_common_subgraph()) == []
    ismags = iso.ISMAGS(graph, nx.Graph())
    assert list(ismags.largest_common_subgraph()) == [{}]


def test_largest_subgraph_empty_graphs():
    graph = nx.empty_graph(1)
    subgraph = nx.empty_graph(1)
    subgraph.nodes[0]["color"] = "red"
    ismags = iso.ISMAGS(graph, subgraph)
    assert list(ismags.largest_common_subgraph()) == [{0: 0}]
    nodematch = nx.isomorphism.categorical_node_match("color", None)
    ismags = iso.ISMAGS(graph, subgraph, node_match=nodematch)
    assert list(ismags.largest_common_subgraph()) == []


def test_largest_subgraph_color_mismatches():
    graph = nx.path_graph(5)
    subgraph = nx.path_graph(5)
    ismags = iso.ISMAGS(graph, subgraph)
    assert list(ismags.largest_common_subgraph()) == [{i: i for i in subgraph}]
    for n in subgraph:
        subgraph.nodes[n]["color"] = "blue"
    nodematch = nx.isomorphism.categorical_node_match("color", None)
    ismags = iso.ISMAGS(graph, subgraph, node_match=nodematch)
    assert list(ismags.largest_common_subgraph()) == []
    for n in graph:
        graph.add_edge(n, n)
    ismags = iso.ISMAGS(graph, subgraph)
    assert list(ismags.largest_common_subgraph(symmetry=False)) == []
