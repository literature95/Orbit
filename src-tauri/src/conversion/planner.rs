//! 最短路寻路：格式作节点，引擎声明的直接边作带权边。
//!
//! 为什么用图搜索：引擎只知道直接转换（例如 mp4 → mp3），
//! 但很多有用转换需要多跳。把格式当节点、边带 cost，
//! 新增一个引擎就能自动解锁一片新转换，无需改动规划器。

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::sync::Arc;

use super::model::{ANY_FORMAT, ConversionEdge, Converter};

/// 一跳的规划结果（未执行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RouteStep {
    pub(crate) converter_id: &'static str,
    pub(crate) from: String,
    pub(crate) to: String,
}

/// 一条完整路由。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub(crate) steps: Vec<RouteStep>,
    pub(crate) cost: u32,
}

pub(crate) struct RoutePlanner<'a> {
    converters: &'a [Arc<dyn Converter>],
    max_steps: usize,
}

impl<'a> RoutePlanner<'a> {
    pub(crate) fn new(converters: &'a [Arc<dyn Converter>]) -> Self {
        Self { converters, max_steps: 3 }
    }

    /// 取某格式出发的所有可用边，通配边对所有源生效。
    fn edges_from(&self, from: &str) -> Vec<(Arc<dyn Converter>, ConversionEdge)> {
        let mut out = Vec::new();
        for converter in self.converters {
            for edge in converter.edges() {
                if edge.from == from || edge.from == ANY_FORMAT {
                    out.push((Arc::clone(converter), *edge));
                }
            }
        }
        out
    }

    /// Dijkstra 求最便宜路由；找不到返回 `None`。
    pub(crate) fn plan(&self, from: &str, to: &str) -> Option<Route> {
        let from = from.to_ascii_lowercase();
        let to = to.to_ascii_lowercase();

        let mut dist: HashMap<String, u32> = HashMap::new();
        let mut hops: HashMap<String, usize> = HashMap::new();
        let mut prev: HashMap<String, RouteStep> = HashMap::new();
        let mut queue: BinaryHeap<Reverse<(u32, String)>> = BinaryHeap::new();

        dist.insert(from.clone(), 0);
        hops.insert(from.clone(), 0);
        queue.push(Reverse((0, from.clone())));

        while let Some(Reverse((cost, node))) = queue.pop() {
            if cost > dist.get(&node).copied().unwrap_or(u32::MAX) {
                continue; // 过期队列项
            }
            if node == to {
                break;
            }
            if hops.get(&node).copied().unwrap_or(usize::MAX) >= self.max_steps {
                continue; // 长链几乎总是有损且慢
            }

            for (converter, edge) in self.edges_from(&node) {
                // 独占边只能作为完整路由，不能当中间跳。
                if edge.direct_only && (node != from || edge.to != to) {
                    continue;
                }
                // 排除自环，避免原地打转；仅「同格式重编码」（from == to）允许首跳自环。
                if edge.to == node && !(node == from && from == to) {
                    continue;
                }
                let next = cost.saturating_add(edge.cost);
                if dist.get(edge.to).is_some_and(|known| *known <= next) {
                    continue;
                }
                dist.insert(edge.to.to_string(), next);
                hops.insert(edge.to.to_string(), hops[&node] + 1);
                prev.insert(
                    edge.to.to_string(),
                    RouteStep {
                        converter_id: converter.id(),
                        from: node.clone(),
                        to: edge.to.to_string(),
                    },
                );
                queue.push(Reverse((next, edge.to.to_string())));
            }
        }

        let mut steps = Vec::new();
        let mut node = to.clone();
        while node != from {
            let step = prev.remove(&node)?;
            node = step.from.clone();
            steps.push(step);
        }
        steps.reverse();
        Some(Route { steps, cost: *dist.get(&to)? })
    }

    /// 所有可到达的目标格式（去重排序），供 `convert.formats` 暴露能力。
    pub(crate) fn targets(&self) -> Vec<String> {
        let mut set = BTreeSet::new();
        for converter in self.converters {
            for edge in converter.edges() {
                set.insert(edge.to.to_string());
            }
        }
        set.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::model::{ConversionEdge, ConversionStep, Converter, ConverterStatus};
    use std::path::PathBuf;

    struct FakeConverter {
        id: &'static str,
        edges: &'static [ConversionEdge],
    }

    impl Converter for FakeConverter {
        fn id(&self) -> &'static str { self.id }
        fn display_name(&self) -> &'static str { self.id }
        fn edges(&self) -> &'static [ConversionEdge] { self.edges }
        fn status(&self) -> ConverterStatus { ConverterStatus::Available { version: None } }
        fn convert(&self, _: &ConversionStep, _: &dyn Fn(f32)) -> Result<Vec<PathBuf>, String> {
            Ok(vec![])
        }
    }

    static EDGES: &[ConversionEdge] = &[
        ConversionEdge::new("a", "b"),
        ConversionEdge::new("b", "c"),
        ConversionEdge::new("b", "d"),
    ];

    fn planner<'a>(converters: &'a [Arc<dyn Converter>]) -> RoutePlanner<'a> {
        RoutePlanner::new(converters)
    }

    #[test]
    fn plans_a_direct_route() {
        let converters: Vec<Arc<dyn Converter>> = vec![Arc::new(FakeConverter { id: "x", edges: EDGES })];
        let route = planner(&converters).plan("a", "b").unwrap();
        assert_eq!(route.steps.len(), 1);
        assert_eq!(route.cost, 10);
        assert_eq!(route.steps[0].converter_id, "x");
    }

    #[test]
    fn plans_the_cheapest_multihop_route() {
        let converters: Vec<Arc<dyn Converter>> = vec![Arc::new(FakeConverter { id: "x", edges: EDGES })];
        let route = planner(&converters).plan("a", "c").unwrap();
        assert_eq!(route.steps.len(), 2);
        assert_eq!(route.cost, 20);
        assert_eq!(route.steps[0].to, "b");
        assert_eq!(route.steps[1].to, "c");
    }

    #[test]
    fn no_route_for_unknown_target() {
        let converters: Vec<Arc<dyn Converter>> = vec![Arc::new(FakeConverter { id: "x", edges: EDGES })];
        assert!(planner(&converters).plan("a", "zzz").is_none());
    }

    #[test]
    fn direct_only_edge_is_never_an_intermediate_hop() {
        static EDGES: &[ConversionEdge] = &[
            ConversionEdge::new("a", "b"),
            ConversionEdge { from: "b", to: "c", cost: 10, direct_only: true },
        ];
        let converters: Vec<Arc<dyn Converter>> = vec![Arc::new(FakeConverter { id: "x", edges: EDGES })];
        assert!(planner(&converters).plan("a", "c").is_none());
        assert_eq!(planner(&converters).plan("b", "c").unwrap().steps.len(), 1);
    }
}
