"""Hidden grader for code-betweenness-k: the two regression tests upstream
added with the fix (networkx a802a27), which exercise public functions only."""

import networkx as nx


def test_edge_betweenness_k():
    G = nx.path_graph(3)
    # k=2, seed=42 samples source nodes 0 and 2; each edge lies on three
    # sampled shortest paths, halved for an undirected graph, scaled by n/k.
    eb = nx.edge_betweenness_centrality(G, k=2, seed=42, normalized=False)
    assert eb == {(0, 1): 9 / 4, (1, 2): 9 / 4}
    eb = nx.edge_betweenness_centrality(G, k=2, seed=42, normalized=True)
    assert eb == {(0, 1): 3 / 4, (1, 2): 3 / 4}


def test_equivalence_non_subset():
    G = nx.path_graph(10, create_using=nx.DiGraph)
    assert nx.betweenness_centrality(G) == nx.betweenness_centrality_subset(
        G, sources=G.nodes(), targets=G.nodes(), normalized=True
    )
    assert nx.edge_betweenness_centrality(G) == nx.edge_betweenness_centrality_subset(
        G, sources=G.nodes(), targets=G.nodes(), normalized=True
    )
