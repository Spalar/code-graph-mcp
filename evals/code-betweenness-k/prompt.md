---
description: "Bug fix from a symptom report: sampled (k < n) edge betweenness and subset betweenness scale differently from the full computation (upstream networkx a802a27, reverted). The fix is in shared rescale helpers across two modules."
max_turns: 100
timeout_seconds: 1800
allowed_tools: [Read, Glob, Grep, Bash, Edit, Write, Agent]
tags: [coding]
---

Bug report for this networkx checkout:

1. `nx.edge_betweenness_centrality(G, k=k, seed=seed)` scales its result wrong when it samples `k < n` source nodes. For `G = nx.path_graph(3)`, `k=2`, `seed=42` (which samples nodes 0 and 2), each edge lies on 3 of the sampled shortest paths; halved because the graph is undirected and scaled up by n/k = 3/2, I expect `{(0, 1): 9/4, (1, 2): 9/4}` with `normalized=False`, and `{(0, 1): 3/4, (1, 2): 3/4}` with `normalized=True`. Node betweenness gets this right.
2. With every node as both source and target, `nx.betweenness_centrality_subset(G, sources=G.nodes(), targets=G.nodes(), normalized=True)` should equal `nx.betweenness_centrality(G)`, and the edge versions should match each other too. They don't for `G = nx.path_graph(10, create_using=nx.DiGraph)`.

Please fix the scaling so all of these agree.
