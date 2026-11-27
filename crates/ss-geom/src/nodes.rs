//! Node view of a Bézier path for the Edit Points tool.
//!
//! A [`BezPath`] is converted to anchors that each carry an optional incoming and outgoing
//! cubic handle. Moving anchors, dragging handles, inserting and deleting nodes are simple
//! list edits in this form; [`NodePath::to_bez`] converts back. Quadratic segments are raised
//! to cubics on the way in.

use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveNearest, PathEl, Point};

const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub pos: Point,
    /// Control point of the segment arriving at this node.
    pub hin: Option<Point>,
    /// Control point of the segment leaving this node.
    pub hout: Option<Point>,
}

impl Node {
    fn at(pos: Point) -> Self {
        Self { pos, hin: None, hout: None }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubPath {
    pub nodes: Vec<Node>,
    pub closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId {
    pub sub: usize,
    pub index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleSide {
    In,
    Out,
}

/// What a pointer press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Anchor(NodeId),
    Handle(NodeId, HandleSide),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NodePath {
    pub subs: Vec<SubPath>,
}

fn real_handle(anchor: Point, h: Point) -> Option<Point> {
    (h.distance(anchor) > EPS).then_some(h)
}

impl NodePath {
    pub fn from_bez(path: &BezPath) -> Self {
        let mut subs = Vec::new();
        let mut cur: Option<SubPath> = None;
        let finish = |cur: &mut Option<SubPath>, subs: &mut Vec<SubPath>, closed: bool| {
            if let Some(mut s) = cur.take() {
                s.closed = closed;
                if closed && s.nodes.len() > 1 {
                    // A closing curve that ends on the start point is the same node.
                    let first = s.nodes[0].pos;
                    if s.nodes.last().is_some_and(|l| l.pos.distance(first) <= EPS) {
                        let last = s.nodes.pop().unwrap_or(Node::at(first));
                        s.nodes[0].hin = last.hin;
                    }
                }
                subs.push(s);
            }
        };
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    finish(&mut cur, &mut subs, false);
                    cur = Some(SubPath { nodes: vec![Node::at(p)], closed: false });
                }
                PathEl::LineTo(p) => {
                    if let Some(c) = cur.as_mut() {
                        c.nodes.push(Node::at(p));
                    }
                }
                PathEl::QuadTo(q, p) => {
                    if let Some(c) = cur.as_mut() {
                        let prev = c.nodes.last().map_or(p, |n| n.pos);
                        let c1 = prev + (q - prev) * (2.0 / 3.0);
                        let c2 = p + (q - p) * (2.0 / 3.0);
                        if let Some(l) = c.nodes.last_mut() {
                            l.hout = real_handle(l.pos, c1);
                        }
                        c.nodes.push(Node { pos: p, hin: real_handle(p, c2), hout: None });
                    }
                }
                PathEl::CurveTo(c1, c2, p) => {
                    if let Some(c) = cur.as_mut() {
                        if let Some(l) = c.nodes.last_mut() {
                            l.hout = real_handle(l.pos, c1);
                        }
                        c.nodes.push(Node { pos: p, hin: real_handle(p, c2), hout: None });
                    }
                }
                PathEl::ClosePath => finish(&mut cur, &mut subs, true),
            }
        }
        finish(&mut cur, &mut subs, false);
        Self { subs }
    }

    pub fn to_bez(&self) -> BezPath {
        let mut out = BezPath::new();
        for s in &self.subs {
            let Some(first) = s.nodes.first() else { continue };
            out.move_to(first.pos);
            for w in s.nodes.windows(2) {
                push_segment(&mut out, &w[0], &w[1]);
            }
            if s.closed && s.nodes.len() > 1 {
                let last = &s.nodes[s.nodes.len() - 1];
                if last.hout.is_some() || first.hin.is_some() {
                    push_segment(&mut out, last, first);
                }
                out.close_path();
            }
        }
        out
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.subs.get(id.sub)?.nodes.get(id.index)
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.subs.get_mut(id.sub)?.nodes.get_mut(id.index)
    }

    /// Anchor under `pt`, or a handle of the `selected` node, within `tol` mm. Handles win over
    /// anchors so a handle lying on its anchor stays draggable only through the anchor.
    pub fn pick(&self, pt: Point, tol: f64, selected: Option<NodeId>) -> Option<Pick> {
        if let Some(id) = selected
            && let Some(n) = self.node(id)
        {
            for (side, h) in [(HandleSide::In, n.hin), (HandleSide::Out, n.hout)] {
                if h.is_some_and(|h| h.distance(pt) <= tol) {
                    return Some(Pick::Handle(id, side));
                }
            }
        }
        let mut best: Option<(f64, NodeId)> = None;
        for (si, s) in self.subs.iter().enumerate() {
            for (ni, n) in s.nodes.iter().enumerate() {
                let d = n.pos.distance(pt);
                if d <= tol && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, NodeId { sub: si, index: ni }));
                }
            }
        }
        best.map(|(_, id)| Pick::Anchor(id))
    }

    /// Moves an anchor and its handles together.
    pub fn move_anchor(&mut self, id: NodeId, to: Point) {
        if let Some(n) = self.node_mut(id) {
            let d = to - n.pos;
            n.pos = to;
            n.hin = n.hin.map(|h| h + d);
            n.hout = n.hout.map(|h| h + d);
        }
    }

    /// Drags one handle. A smooth node (handles collinear on opposite sides) keeps its other
    /// handle collinear, preserving that handle's length; a corner node moves only this handle.
    pub fn move_handle(&mut self, id: NodeId, side: HandleSide, to: Point) {
        let Some(n) = self.node_mut(id) else { return };
        let smooth = is_smooth(n);
        let pos = n.pos;
        match side {
            HandleSide::In => n.hin = Some(to),
            HandleSide::Out => n.hout = Some(to),
        }
        let dir = to - pos;
        if smooth && dir.hypot() > EPS {
            let other = match side {
                HandleSide::In => &mut n.hout,
                HandleSide::Out => &mut n.hin,
            };
            if let Some(o) = other {
                let len = o.distance(pos);
                *o = pos - dir.normalize() * len;
            }
        }
    }

    /// Deletes a node; its neighbours are joined by a straight line.
    pub fn delete_node(&mut self, id: NodeId) -> Result<(), &'static str> {
        let Some(s) = self.subs.get_mut(id.sub) else { return Err("No such point") };
        let n = s.nodes.len();
        let min = if s.closed { 4 } else { 3 };
        if id.index >= n {
            return Err("No such point");
        }
        if n < min {
            return Err(if s.closed {
                "A closed path needs at least three points"
            } else {
                "A path needs at least two points"
            });
        }
        let prev = if id.index > 0 { Some(id.index - 1) } else if s.closed { Some(n - 1) } else { None };
        let next = if id.index + 1 < n { Some(id.index + 1) } else if s.closed { Some(0) } else { None };
        if let Some(p) = prev {
            s.nodes[p].hout = None;
        }
        if let Some(q) = next {
            s.nodes[q].hin = None;
        }
        s.nodes.remove(id.index);
        Ok(())
    }

    /// Inserts a node on the segment nearest `pt` if it lies within `tol`, splitting curves so
    /// the shape is unchanged. Returns the new node.
    pub fn insert_at(&mut self, pt: Point, tol: f64) -> Option<NodeId> {
        let mut best: Option<(f64, usize, usize, f64)> = None;
        for (si, s) in self.subs.iter().enumerate() {
            let n = s.nodes.len();
            let segs = if s.closed { n } else { n.saturating_sub(1) };
            for i in 0..segs {
                let near = segment(&s.nodes[i], &s.nodes[(i + 1) % n]).nearest(pt, 1e-4);
                if near.distance_sq.sqrt() <= tol && best.is_none_or(|(d, ..)| near.distance_sq < d) {
                    best = Some((near.distance_sq, si, i, near.t));
                }
            }
        }
        let (_, si, i, t) = best?;
        if !(0.02..=0.98).contains(&t) {
            return None;
        }
        let s = &mut self.subs[si];
        let n = s.nodes.len();
        let j = (i + 1) % n;
        let (a, b) = (s.nodes[i], s.nodes[j]);
        let new = if a.hout.is_none() && b.hin.is_none() {
            Node::at(a.pos.lerp(b.pos, t))
        } else {
            let c = segment(&a, &b);
            let (left, right) = (c.subsegment(0.0..t), c.subsegment(t..1.0));
            s.nodes[i].hout = real_handle(a.pos, left.p1);
            s.nodes[j].hin = real_handle(b.pos, right.p2);
            Node { pos: left.p3, hin: real_handle(left.p3, left.p2), hout: real_handle(left.p3, right.p1) }
        };
        let at = i + 1;
        s.nodes.insert(at, new);
        Some(NodeId { sub: si, index: at })
    }
}

fn is_smooth(n: &Node) -> bool {
    let (Some(a), Some(b)) = (n.hin, n.hout) else { return false };
    let (u, v) = (a - n.pos, b - n.pos);
    let (lu, lv) = (u.hypot(), v.hypot());
    lu > EPS && lv > EPS && (u.cross(v) / (lu * lv)).abs() < 0.02 && u.dot(v) < 0.0
}

fn segment(a: &Node, b: &Node) -> CubicBez {
    CubicBez::new(a.pos, a.hout.unwrap_or(a.pos), b.hin.unwrap_or(b.pos), b.pos)
}

fn push_segment(out: &mut BezPath, a: &Node, b: &Node) {
    if a.hout.is_none() && b.hin.is_none() {
        out.line_to(b.pos);
    } else {
        out.curve_to(a.hout.unwrap_or(a.pos), b.hin.unwrap_or(b.pos), b.pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        p.line_to((0.0, 10.0));
        p.close_path();
        p
    }

    fn smooth_open() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((0.0, 5.0), (5.0, 10.0), (10.0, 10.0));
        p.curve_to((15.0, 10.0), (20.0, 5.0), (20.0, 0.0));
        p
    }

    #[test]
    fn round_trip_keeps_shape() {
        for p in [square(), smooth_open()] {
            assert_eq!(NodePath::from_bez(&p).to_bez(), p);
        }
    }

    #[test]
    fn closing_curve_twin_is_merged() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.curve_to((10.0, 10.0), (0.0, 10.0), (0.0, 0.0));
        p.close_path();
        let np = NodePath::from_bez(&p);
        assert_eq!(np.subs[0].nodes.len(), 2);
        assert!(np.subs[0].nodes[0].hin.is_some());
    }

    #[test]
    fn moving_an_anchor_carries_its_handles() {
        let mut np = NodePath::from_bez(&smooth_open());
        let id = NodeId { sub: 0, index: 1 };
        np.move_anchor(id, Point::new(12.0, 12.0));
        let n = np.node(id).unwrap();
        assert_eq!(n.pos, Point::new(12.0, 12.0));
        assert_eq!(n.hin, Some(Point::new(7.0, 12.0)));
        assert_eq!(n.hout, Some(Point::new(17.0, 12.0)));
    }

    #[test]
    fn smooth_handles_stay_collinear_and_corner_handles_do_not() {
        let mut np = NodePath::from_bez(&smooth_open());
        let id = NodeId { sub: 0, index: 1 };
        np.move_handle(id, HandleSide::Out, Point::new(10.0, 14.0));
        let n = np.node(id).unwrap();
        assert_eq!(n.hout, Some(Point::new(10.0, 14.0)));
        let hin = n.hin.unwrap();
        assert!((hin.x - 10.0).abs() < 1e-9 && (hin.distance(n.pos) - 5.0).abs() < 1e-9 && hin.y < 10.0);

        // Break the node, then the other handle must not follow.
        np.subs[0].nodes[1].hin = Some(Point::new(7.0, 7.0));
        let before = np.node(id).unwrap().hin;
        np.move_handle(id, HandleSide::Out, Point::new(14.0, 12.0));
        assert_eq!(np.node(id).unwrap().hin, before);
    }

    #[test]
    fn deleting_joins_neighbours_with_a_line() {
        let mut np = NodePath::from_bez(&smooth_open());
        np.subs[0].nodes.push(Node::at(Point::new(25.0, 0.0)));
        np.delete_node(NodeId { sub: 0, index: 1 }).unwrap();
        let s = &np.subs[0];
        assert_eq!(s.nodes.len(), 3);
        assert!(s.nodes[0].hout.is_none() && s.nodes[1].hin.is_none());

        // A closed path may shrink to a triangle but no further.
        let mut np = NodePath::from_bez(&square());
        np.delete_node(NodeId { sub: 0, index: 2 }).unwrap();
        assert_eq!(np.subs[0].nodes.len(), 3);
        assert!(np.delete_node(NodeId { sub: 0, index: 0 }).is_err());

        // An open path may shrink to a single segment but no further.
        let mut open = NodePath::from_bez(&smooth_open());
        open.delete_node(NodeId { sub: 0, index: 1 }).unwrap();
        assert_eq!(open.subs[0].nodes.len(), 2);
        assert!(open.delete_node(NodeId { sub: 0, index: 0 }).is_err());
    }

    #[test]
    fn inserting_on_a_curve_preserves_it() {
        let mut np = NodePath::from_bez(&smooth_open());
        let c = segment(&np.subs[0].nodes[0], &np.subs[0].nodes[1]);
        let on = c.eval(0.5);
        let id = np.insert_at(on, 0.05).unwrap();
        assert_eq!(id, NodeId { sub: 0, index: 1 });
        assert_eq!(np.subs[0].nodes.len(), 4);
        let again = segment(&np.subs[0].nodes[0], &np.subs[0].nodes[1]).eval(1.0);
        assert!(again.distance(on) < 1e-9);
        assert!(np.insert_at(Point::new(50.0, 50.0), 0.5).is_none());
    }

    #[test]
    fn inserting_on_a_closing_line_appends() {
        let mut np = NodePath::from_bez(&square());
        let id = np.insert_at(Point::new(0.0, 5.0), 0.1).unwrap();
        assert_eq!(id.index, 4);
        assert!(np.subs[0].nodes[4].pos.distance(Point::new(0.0, 5.0)) < 1e-6);
    }

    #[test]
    fn picking_prefers_selected_handles() {
        let np = NodePath::from_bez(&smooth_open());
        let sel = NodeId { sub: 0, index: 1 };
        assert_eq!(np.pick(Point::new(15.0, 10.0), 0.5, Some(sel)), Some(Pick::Handle(sel, HandleSide::Out)));
        assert_eq!(np.pick(Point::new(15.0, 10.0), 0.5, None), None);
        assert_eq!(np.pick(Point::new(10.1, 10.0), 0.5, None), Some(Pick::Anchor(sel)));
    }
}
