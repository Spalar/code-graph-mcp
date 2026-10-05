---
description: "Feature across a package: callable `capacity` for every max-flow/min-cut entry point (upstream networkx 0080011, reverted). The plumbing sits in a shared helper plus seven algorithm modules."
max_turns: 100
timeout_seconds: 1800
allowed_tools: [Read, Glob, Grep, Bash, Edit, Write, Agent]
tags: [coding]
---

This is the networkx repository. In the shortest-path functions, `weight` can be an edge-attribute name or a function `weight(u, v, d)` that returns the weight of edge (u, v) from its data dict `d`. The maximum-flow / minimum-cut functions don't have that: their `capacity` argument must be an attribute name.

Add support for passing a function `capacity(u, v, d)` that returns the capacity of the edge. It should work everywhere `capacity` is accepted in the max-flow/min-cut family: `maximum_flow`, `maximum_flow_value`, `minimum_cut`, `minimum_cut_value`, every algorithm that can be passed as `flow_func`, and `gomory_hu_tree`. A function that reproduces an attribute's values must give the same results as that attribute's name, and the string form must keep working as it does now. Update the docstrings that describe `capacity`.
