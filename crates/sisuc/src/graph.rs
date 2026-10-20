//! Graph algorithms the checker and mir share.

/// Each node's strongly connected component of the graph `edges`, by Tarjan's algorithm,
/// visiting the nodes in order. Components are numbered as they complete, so a component's
/// number is above that of every other component it reaches. The walk keeps its own stack,
/// since a chain of declarations or of calls can be long.
pub(crate) fn components(edges: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let mut index = vec![UNSEEN; edges.len()];
    let mut low = vec![0; edges.len()];
    let mut component = vec![UNSEEN; edges.len()];
    // The nodes seen and not yet in a component, and the walk's path with each node's next edge.
    let mut open = Vec::new();
    let mut path: Vec<(usize, usize)> = Vec::new();
    let mut next_index = 0;
    let mut next_component = 0;
    for root in 0..edges.len() {
        if index[root] != UNSEEN {
            continue;
        }
        path.push((root, 0));
        while let Some((n, next)) = path.last_mut() {
            let n = *n;
            if index[n] == UNSEEN {
                index[n] = next_index;
                low[n] = next_index;
                next_index += 1;
                open.push(n);
            }
            if let Some(&m) = edges[n].get(*next) {
                *next += 1;
                if index[m] == UNSEEN {
                    path.push((m, 0));
                } else if component[m] == UNSEEN {
                    low[n] = low[n].min(index[m]);
                }
                continue;
            }
            path.pop();
            if let Some(&(parent, _)) = path.last() {
                low[parent] = low[parent].min(low[n]);
            }
            if low[n] == index[n] {
                while let Some(m) = open.pop() {
                    component[m] = next_component;
                    if m == n {
                        break;
                    }
                }
                next_component += 1;
            }
        }
    }
    component
}
