// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Final clipping of route rays to exact rendered shape outlines.
//!
//! Shape-specific intersections replace box/port working endpoints, including
//! D2 rounding behavior and 3D or multiple-object modifiers.

use super::*;
use crate::engine::{ArenaNode, bounds::modifier_element_adjustments};

fn line_intersection_point(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    // Direct translation of D2 geo.IntersectionPoint, which TALA calls while
    // tracing every shape perimeter. It rounds the displacement along the
    // first segment before adding its origin; rounding only the final point is
    // observably different when the segment starts at a fractional position.
    let a_dx = b.x - a.x;
    let c_dx = d.x - c.x;
    let ac_dx = c.x - a.x;
    let a_dy = b.y - a.y;
    let c_dy = d.y - c.y;
    let ac_dy = c.y - a.y;
    let denominator = a_dy * c_dx - a_dx * c_dy;
    if denominator == 0.0 {
        return None;
    }
    let first_t = (c_dx * ac_dy - c_dy * ac_dx) / denominator;
    let second_t = (a_dx * ac_dy - a_dy * ac_dx) / denominator;
    if !(0.0..=1.0).contains(&first_t) || !(0.0..=1.0).contains(&second_t) {
        return None;
    }
    Some(Point {
        x: a.x + (first_t * a_dx).round(),
        y: a.y + (first_t * a_dy).round(),
    })
}

fn extended_shape_ray(previous: Point, border: Point, rect: Rect) -> (Point, Point) {
    let dx = border.x - previous.x;
    let dy = border.y - previous.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= 1e-9 {
        return (previous, border);
    }
    let extension = if dx.abs() <= 1e-9 {
        rect.size.height
    } else {
        rect.size.width
    };
    let scale = (length + extension) / length;
    (
        previous,
        Point {
            x: previous.x + dx * scale,
            y: previous.y + dy * scale,
        },
    )
}

fn trace_polygon(rect: Rect, border: Point, previous: Point, normalized: &[(f64, f64)]) -> Point {
    let (ray_start, ray_end) = extended_shape_ray(previous, border, rect);
    let path_coordinate = |value: f64| ((value * 10_000.0) as f32 as f64 / 10_000.0).round();
    let points: Vec<_> = normalized
        .iter()
        .map(|(x, y)| Point {
            x: path_coordinate(rect.origin.x + x * rect.size.width),
            y: path_coordinate(rect.origin.y + y * rect.size.height),
        })
        .collect();
    let mut closest = border;
    let mut closest_distance = f64::INFINITY;
    for index in 0..points.len() {
        let first = points[index];
        let second = points[(index + 1) % points.len()];
        if let Some(intersection) = line_intersection_point(first, second, ray_start, ray_end) {
            let distance =
                ((intersection.x - border.x).powi(2) + (intersection.y - border.y).powi(2)).sqrt();
            if distance < closest_distance {
                closest = intersection;
                closest_distance = distance;
            }
        }
    }
    Point {
        x: (closest.x as f32 as f64).round(),
        y: (closest.y as f32 as f64).round(),
    }
}

fn trace_pixel_polygon(
    rect: Rect,
    border: Point,
    previous: Point,
    local_points: &[(f64, f64)],
) -> Point {
    let (ray_start, ray_end) = extended_shape_ray(previous, border, rect);
    let path_coordinate = |value: f64| ((value * 10_000.0) as f32 as f64 / 10_000.0).round();
    let points: Vec<_> = local_points
        .iter()
        .map(|(x, y)| Point {
            x: path_coordinate(rect.origin.x + x),
            y: path_coordinate(rect.origin.y + y),
        })
        .collect();
    let mut closest = border;
    let mut closest_distance = f64::INFINITY;
    for index in 0..points.len() {
        let first = points[index];
        let second = points[(index + 1) % points.len()];
        if let Some(intersection) = line_intersection_point(first, second, ray_start, ray_end) {
            let distance =
                ((intersection.x - border.x).powi(2) + (intersection.y - border.y).powi(2)).sqrt();
            if distance < closest_distance {
                closest = intersection;
                closest_distance = distance;
            }
        }
    }
    Point {
        x: (closest.x as f32 as f64).round(),
        y: (closest.y as f32 as f64).round(),
    }
}

fn ellipse_intersections(
    center: Point,
    radius_x: f64,
    radius_y: f64,
    ray: (Point, Point),
) -> Vec<Point> {
    if radius_x <= 0.0 || radius_y <= 0.0 {
        return Vec::new();
    }
    let dx = ray.1.x - ray.0.x;
    let dy = ray.1.y - ray.0.y;
    let ox = ray.0.x - center.x;
    let oy = ray.0.y - center.y;
    let a = dx * dx / (radius_x * radius_x) + dy * dy / (radius_y * radius_y);
    let b = 2.0 * (ox * dx / (radius_x * radius_x) + oy * dy / (radius_y * radius_y));
    let c = ox * ox / (radius_x * radius_x) + oy * oy / (radius_y * radius_y) - 1.0;
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 || a.abs() <= 1e-9 {
        return Vec::new();
    }
    let mut intersections = Vec::new();
    for t in [
        (-b - discriminant.sqrt()) / (2.0 * a),
        (-b + discriminant.sqrt()) / (2.0 * a),
    ] {
        if !(-1e-9..=1.0 + 1e-9).contains(&t) {
            continue;
        }
        intersections.push(Point {
            x: ray.0.x + t * dx,
            y: ray.0.y + t * dy,
        });
    }
    intersections
}

fn rounded_closest(border: Point, intersections: impl IntoIterator<Item = Point>) -> Point {
    let mut closest = border;
    let mut closest_distance = f64::INFINITY;
    for point in intersections {
        let distance = ((point.x - border.x).powi(2) + (point.y - border.y).powi(2)).sqrt();
        if distance < closest_distance {
            closest = point;
            closest_distance = distance;
        }
    }
    Point {
        x: (closest.x as f32 as f64).round(),
        y: (closest.y as f32 as f64).round(),
    }
}

fn trace_ellipse(rect: Rect, border: Point, previous: Point) -> Point {
    let ray = extended_shape_ray(previous, border, rect);
    let center = Point {
        x: rect.origin.x + rect.size.width * 0.5,
        y: rect.origin.y + rect.size.height * 0.5,
    };
    rounded_closest(
        border,
        ellipse_intersections(center, rect.size.width * 0.5, rect.size.height * 0.5, ray),
    )
}

#[derive(Clone, Copy)]
enum ShapePathSegment {
    Line(Point, Point),
    Cubic([Point; 4]),
}

struct ShapePath {
    segments: Vec<ShapePathSegment>,
    top_left: Point,
    scale_x: f64,
    scale_y: f64,
    start: Point,
    current: Point,
}

impl ShapePath {
    fn new(top_left: Point, scale_x: f64, scale_y: f64) -> Self {
        Self {
            segments: Vec::new(),
            top_left,
            scale_x,
            scale_y,
            start: top_left,
            current: top_left,
        }
    }

    /// D2's SVG path builder deliberately reduces each scaled coordinate to
    /// float32 before rounding the resulting coordinate so shape geometry is
    /// stable across architectures.
    fn chop_precision(value: f64) -> f64 {
        ((value * 10_000.0) as f32 as f64 / 10_000.0).round()
    }

    fn absolute(&self, x: f64, y: f64) -> Point {
        Point {
            x: Self::chop_precision(self.top_left.x + self.scale_x * x),
            y: Self::chop_precision(self.top_left.y + self.scale_y * y),
        }
    }

    fn relative(&self, x: f64, y: f64) -> Point {
        Point {
            x: Self::chop_precision(self.current.x + self.scale_x * x),
            y: Self::chop_precision(self.current.y + self.scale_y * y),
        }
    }

    fn start_at(&mut self, point: Point) {
        self.start = point;
        self.current = point;
    }

    fn horizontal(&mut self, relative: bool, x: f64) {
        let mut end = if relative {
            self.relative(x, 0.0)
        } else {
            self.absolute(x, 0.0)
        };
        end.y = self.current.y;
        self.segments
            .push(ShapePathSegment::Line(self.current, end));
        self.current = end;
    }

    fn vertical(&mut self, relative: bool, y: f64) {
        let mut end = if relative {
            self.relative(0.0, y)
        } else {
            self.absolute(0.0, y)
        };
        end.x = self.current.x;
        self.segments
            .push(ShapePathSegment::Line(self.current, end));
        self.current = end;
    }

    fn line(&mut self, relative: bool, x: f64, y: f64) {
        let end = if relative {
            self.relative(x, y)
        } else {
            self.absolute(x, y)
        };
        self.segments
            .push(ShapePathSegment::Line(self.current, end));
        self.current = end;
    }

    fn cubic(&mut self, relative: bool, values: [f64; 6]) {
        let point = |path: &Self, x, y| {
            if relative {
                path.relative(x, y)
            } else {
                path.absolute(x, y)
            }
        };
        let controls = [
            self.current,
            point(self, values[0], values[1]),
            point(self, values[2], values[3]),
            point(self, values[4], values[5]),
        ];
        self.segments.push(ShapePathSegment::Cubic(controls));
        self.current = controls[3];
    }

    fn close(&mut self) {
        self.segments
            .push(ShapePathSegment::Line(self.current, self.start));
        self.current = self.start;
    }
}

fn bezier_coefficients(p0: f64, p1: f64, p2: f64, p3: f64) -> [f64; 4] {
    [
        -p0 + 3.0 * p1 - 3.0 * p2 + p3,
        3.0 * p0 - 6.0 * p1 + 3.0 * p2,
        -3.0 * p0 + 3.0 * p1,
        p0,
    ]
}

fn precision_compare(left: f64, right: f64, precision: f64) -> std::cmp::Ordering {
    if (left - right).abs() < precision {
        std::cmp::Ordering::Equal
    } else if left < right {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Greater
    }
}

fn sort_special_roots(mut roots: [f64; 3]) -> [f64; 3] {
    loop {
        let mut flipped = false;
        for index in 0..2 {
            let next_nonnegative = precision_compare(roots[index + 1], 0.0, 0.0001).is_ge();
            let current_greater = precision_compare(roots[index], roots[index + 1], 0.0001).is_gt();
            let current_negative = precision_compare(roots[index], 0.0, 0.0001).is_lt();
            if next_nonnegative && (current_greater || current_negative) {
                roots.swap(index, index + 1);
                flipped = true;
            }
        }
        if !flipped {
            return roots;
        }
    }
}

/// Direct port of the cubic root ordering and precision behavior used by the
/// D2 geometry dependency bundled with TALA.
fn cubic_roots(polynomial: [f64; 4]) -> [f64; 3] {
    const PRECISION: f64 = 0.0001;
    if precision_compare(polynomial[0], 0.0, PRECISION).is_eq() {
        if precision_compare(polynomial[1], 0.0, PRECISION).is_eq() {
            let mut roots = [-polynomial[3] / polynomial[2], -1.0, -1.0];
            if precision_compare(roots[0], 0.0, PRECISION).is_lt()
                || precision_compare(roots[0], 1.0, PRECISION).is_gt()
            {
                roots[0] = -1.0;
            }
            return sort_special_roots(roots);
        }

        let mut discriminant = polynomial[2].powi(2) - 4.0 * polynomial[1] * polynomial[3];
        if precision_compare(discriminant, 0.0, PRECISION).is_ge() {
            discriminant = discriminant.sqrt();
            let mut roots = [
                -(discriminant + polynomial[2]) / (2.0 * polynomial[1]),
                (discriminant - polynomial[2]) / (2.0 * polynomial[1]),
                -1.0,
            ];
            for root in roots.iter_mut().take(2) {
                if precision_compare(*root, 0.0, PRECISION).is_lt()
                    || precision_compare(*root, 1.0, PRECISION).is_gt()
                {
                    *root = -1.0;
                }
            }
            return sort_special_roots(roots);
        }
    }

    let a = polynomial[1] / polynomial[0];
    let b = polynomial[2] / polynomial[0];
    let c = polynomial[3] / polynomial[0];
    let q = (3.0 * b - a.powi(2)) / 9.0;
    let r = (9.0 * a * b - 27.0 * c - 2.0 * a.powi(3)) / 54.0;
    let discriminant = q.powi(3) + r.powi(2);
    let mut roots;
    if precision_compare(discriminant, 0.0, PRECISION).is_ge() {
        let signed_cube_root = |value: f64| value.signum() * value.abs().powf(1.0 / 3.0);
        let s = signed_cube_root(r + discriminant.sqrt());
        let t = signed_cube_root(r - discriminant.sqrt());
        roots = [
            -a / 3.0 + s + t,
            -a / 3.0 - (s + t) / 2.0,
            -a / 3.0 - (s + t) / 2.0,
        ];
        let imaginary = (3.0_f64).sqrt() * (s - t).abs() / 2.0;
        if !precision_compare(imaginary, 0.0, PRECISION).is_eq() {
            roots[1] = -1.0;
            roots[2] = -1.0;
        }
    } else {
        let theta = (r / (-q.powi(3)).sqrt()).acos();
        roots = [
            2.0 * (-q).sqrt() * (theta / 3.0).cos() - a / 3.0,
            2.0 * (-q).sqrt() * ((theta + 2.0 * std::f64::consts::PI) / 3.0).cos() - a / 3.0,
            2.0 * (-q).sqrt() * ((theta + 4.0 * std::f64::consts::PI) / 3.0).cos() - a / 3.0,
        ];
    }
    for root in &mut roots {
        if precision_compare(*root, 0.0, PRECISION).is_lt()
            || precision_compare(*root, 1.0, PRECISION).is_gt()
        {
            *root = -1.0;
        }
    }
    sort_special_roots(roots)
}

fn cubic_line_intersections(curve: [Point; 4], line: (Point, Point)) -> Vec<Point> {
    const PRECISION: f64 = 0.0001;
    let line_a = line.1.y - line.0.y;
    let line_b = line.0.x - line.1.x;
    let line_c = line.0.x * (line.0.y - line.1.y) + line.0.y * (line.1.x - line.0.x);
    let x = bezier_coefficients(curve[0].x, curve[1].x, curve[2].x, curve[3].x);
    let y = bezier_coefficients(curve[0].y, curve[1].y, curve[2].y, curve[3].y);
    let roots = cubic_roots([
        line_a * x[0] + line_b * y[0],
        line_a * x[1] + line_b * y[1],
        line_a * x[2] + line_b * y[2],
        line_a * x[3] + line_b * y[3] + line_c,
    ]);
    let mut intersections = Vec::new();
    for parameter in roots {
        let point = Point {
            x: x[0] * parameter.powi(3) + x[1] * parameter.powi(2) + x[2] * parameter + x[3],
            y: y[0] * parameter.powi(3) + y[1] * parameter.powi(2) + y[2] * parameter + y[3],
        };
        let line_parameter = if line.1.x != line.0.x {
            (point.x - line.0.x) / (line.1.x - line.0.x)
        } else {
            (point.y - line.0.y) / (line.1.y - line.0.y)
        };
        if !precision_compare(parameter, 0.0, PRECISION).is_lt()
            && !precision_compare(parameter, 1.0, PRECISION).is_gt()
            && !precision_compare(line_parameter, 0.0, PRECISION).is_lt()
            && !precision_compare(line_parameter, 1.0, PRECISION).is_gt()
        {
            intersections.push(point);
        }
    }
    intersections
}

fn shape_path_intersections(path: ShapePath, ray: (Point, Point)) -> Vec<Point> {
    let mut intersections = Vec::new();
    for segment in path.segments {
        intersections.extend(match segment {
            ShapePathSegment::Line(start, end) => line_intersection_point(start, end, ray.0, ray.1)
                .into_iter()
                .collect(),
            ShapePathSegment::Cubic(curve) => cubic_line_intersections(curve, ray),
        });
    }
    intersections
}

fn trace_shape_path(rect: Rect, border: Point, previous: Point, path: ShapePath) -> Point {
    let ray = extended_shape_ray(previous, border, rect);
    rounded_closest(border, shape_path_intersections(path, ray))
}

/// Direct port of D2's `shapeDiamond.diamondPath`.
fn diamond_path(rect: Rect) -> ShapePath {
    let mut path = ShapePath::new(rect.origin, rect.size.width / 77.0, rect.size.height / 76.9);
    path.start_at(path.absolute(38.5, 76.9));
    path.cubic(true, [-0.3, 0.0, -0.5, -0.1, -0.7, -0.3]);
    path.line(false, 0.3, 39.2);
    path.cubic(true, [-0.4, -0.4, -0.4, -1.0, 0.0, -1.4]);
    path.line(false, 37.8, 0.3);
    path.cubic(true, [0.4, -0.4, 1.0, -0.4, 1.4, 0.0]);
    path.line(true, 37.5, 37.5);
    path.cubic(true, [0.4, 0.4, 0.4, 1.0, 0.0, 1.4]);
    path.line(false, 39.2, 76.6);
    path.cubic(false, [39.0, 76.8, 38.8, 76.9, 38.5, 76.9]);
    path.close();
    path
}

/// Direct port of D2's `shapeCallout.calloutPath`.
fn callout_path(rect: Rect) -> ShapePath {
    let tip_width = if rect.size.width < 60.0 {
        rect.size.width / 2.0
    } else {
        30.0
    };
    let tip_height = if rect.size.height < 90.0 {
        rect.size.height / 2.0
    } else {
        45.0
    };
    let body_height = rect.size.height - tip_height;
    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(0.0, 0.0));
    path.vertical(true, body_height);
    path.horizontal(true, rect.size.width / 2.0);
    path.vertical(true, tip_height);
    path.line(true, tip_width, -tip_height);
    path.horizontal(true, rect.size.width / 2.0 - tip_width);
    path.vertical(true, -body_height);
    path.horizontal(true, -rect.size.width);
    path.close();
    path
}

fn queue_path(rect: Rect) -> ShapePath {
    let arc = 24.0_f64.min(rect.size.width * 0.5);
    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(arc, 0.0));
    path.horizontal(true, rect.size.width - 2.0 * arc);
    path.cubic(
        false,
        [
            rect.size.width,
            0.0,
            rect.size.width,
            rect.size.height * 0.45,
            rect.size.width,
            rect.size.height * 0.5,
        ],
    );
    path.cubic(
        false,
        [
            rect.size.width,
            rect.size.height * 0.55,
            rect.size.width,
            rect.size.height,
            rect.size.width - arc,
            rect.size.height,
        ],
    );
    path.horizontal(true, -(rect.size.width - 2.0 * arc));
    path.cubic(
        false,
        [
            0.0,
            rect.size.height,
            0.0,
            rect.size.height * 0.55,
            0.0,
            rect.size.height * 0.5,
        ],
    );
    path.cubic(false, [0.0, rect.size.height * 0.45, 0.0, 0.0, arc, 0.0]);
    path.close();
    path
}

fn cylinder_path(rect: Rect) -> ShapePath {
    let arc = 24.0_f64.min(rect.size.height * 0.5);
    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(0.0, arc));
    path.cubic(
        false,
        [
            0.0,
            0.0,
            rect.size.width * 0.45,
            0.0,
            rect.size.width * 0.5,
            0.0,
        ],
    );
    path.cubic(
        false,
        [
            rect.size.width * 0.55,
            0.0,
            rect.size.width,
            0.0,
            rect.size.width,
            arc,
        ],
    );
    path.vertical(true, rect.size.height - 2.0 * arc);
    path.cubic(
        false,
        [
            rect.size.width,
            rect.size.height,
            rect.size.width * 0.55,
            rect.size.height,
            rect.size.width * 0.5,
            rect.size.height,
        ],
    );
    path.cubic(
        false,
        [
            rect.size.width * 0.45,
            rect.size.height,
            0.0,
            rect.size.height,
            0.0,
            rect.size.height - arc,
        ],
    );
    path.vertical(true, -(rect.size.height - 2.0 * arc));
    path.close();
    path
}

/// Exact `storedDataPath` from the D2 shape library linked into TALA.
fn stored_data_path(rect: Rect) -> ShapePath {
    let mut wedge_width = 15.0;
    const MULTIPLIER: f64 = 0.27;
    if rect.size.width < wedge_width * 2.0 {
        wedge_width = rect.size.width * 0.5;
    }

    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(wedge_width, 0.0));
    path.horizontal(true, rect.size.width - wedge_width);
    path.cubic(
        false,
        [
            rect.size.width - wedge_width * MULTIPLIER,
            0.0,
            rect.size.width - wedge_width,
            rect.size.height * MULTIPLIER,
            rect.size.width - wedge_width,
            rect.size.height * 0.5,
        ],
    );
    path.cubic(
        false,
        [
            rect.size.width - wedge_width,
            rect.size.height - rect.size.height * MULTIPLIER,
            rect.size.width - wedge_width * MULTIPLIER,
            rect.size.height,
            rect.size.width,
            rect.size.height,
        ],
    );
    path.horizontal(true, -(rect.size.width - wedge_width));
    path.cubic(
        false,
        [
            wedge_width - wedge_width * MULTIPLIER,
            rect.size.height,
            0.0,
            rect.size.height - rect.size.height * MULTIPLIER,
            0.0,
            rect.size.height * 0.5,
        ],
    );
    path.cubic(
        false,
        [
            0.0,
            rect.size.height * MULTIPLIER,
            wedge_width - wedge_width * MULTIPLIER,
            0.0,
            wedge_width,
            0.0,
        ],
    );
    path.close();
    path
}

/// Exact outer page perimeter from the D2 shape library linked into TALA.
fn page_outer_path(rect: Rect) -> ShapePath {
    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(0.5, 0.0));
    path.horizontal(false, rect.size.width - 20.8164);
    path.cubic(
        false,
        [
            rect.size.width - 19.6456,
            0.0,
            rect.size.width - 18.521,
            0.456297,
            rect.size.width - 17.6811,
            1.27202,
        ],
    );
    path.line(false, rect.size.width - 1.3647, 17.12);
    path.cubic(
        false,
        [
            rect.size.width - 0.4923,
            17.9674,
            rect.size.width,
            19.1318,
            rect.size.width,
            20.348,
        ],
    );
    path.vertical(false, rect.size.height - 0.5);
    path.cubic(
        false,
        [
            rect.size.width,
            rect.size.height - 0.2239,
            rect.size.width - 0.2239,
            rect.size.height,
            rect.size.width - 0.5,
            rect.size.height,
        ],
    );
    path.horizontal(false, 0.499999);
    path.cubic(
        false,
        [
            0.223857,
            rect.size.height,
            0.0,
            rect.size.height - 0.2239,
            0.0,
            rect.size.height - 0.5,
        ],
    );
    path.vertical(false, 0.499999);
    path.cubic(false, [0.0, 0.223857, 0.223857, 0.0, 0.5, 0.0]);
    path.close();
    path
}

/// Exact document perimeter from the D2 shape library linked into TALA.
fn document_path(rect: Rect) -> ShapePath {
    const PATH_HEIGHT: f64 = 18.925;
    const PATH_BOTTOM: f64 = 16.3;

    let mut path = ShapePath::new(rect.origin, rect.size.width, rect.size.height);
    path.start_at(path.absolute(0.0, PATH_BOTTOM / PATH_HEIGHT));
    path.line(false, 0.0, 0.0);
    path.line(false, 1.0, 0.0);
    path.line(false, 1.0, PATH_BOTTOM / PATH_HEIGHT);
    path.cubic(
        false,
        [
            5.0 / 6.0,
            12.8 / PATH_HEIGHT,
            2.0 / 3.0,
            12.8 / PATH_HEIGHT,
            0.5,
            PATH_BOTTOM / PATH_HEIGHT,
        ],
    );
    path.cubic(
        false,
        [
            1.0 / 3.0,
            19.8 / PATH_HEIGHT,
            1.0 / 6.0,
            19.8 / PATH_HEIGHT,
            0.0,
            PATH_BOTTOM / PATH_HEIGHT,
        ],
    );
    path.close();
    path
}

/// Exact package perimeter from the D2 shape library linked into TALA.
fn package_path(rect: Rect) -> ShapePath {
    let mut top_width = rect.size.width * 0.5;
    if rect.size.width >= 100.0 {
        top_width = top_width.clamp(50.0, 150.0);
    }
    let top_height = 55.0_f64.min(rect.size.height * 0.2);

    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(0.0, 0.0));
    path.line(false, top_width, 0.0);
    path.line(false, top_width, top_height);
    path.line(false, rect.size.width, top_height);
    path.line(false, rect.size.width, rect.size.height);
    path.line(false, 0.0, rect.size.height);
    path.close();
    path
}

fn c4_person_body_path(rect: Rect) -> ShapePath {
    const HEAD_RADIUS_FACTOR: f64 = 0.22;
    const BODY_TOP_FACTOR: f64 = 0.8;
    const CORNER_RADIUS_FACTOR: f64 = 0.175;

    let head_radius = rect.size.width * HEAD_RADIUS_FACTOR;
    let body_top = head_radius + head_radius * BODY_TOP_FACTOR;
    let body_height = rect.size.height - body_top;
    let corner_radius = (rect.size.width * CORNER_RADIUS_FACTOR).min(body_height * 0.25);
    let curve_factor = 4.0 * (2.0_f64.sqrt() - 1.0) / 3.0;

    let mut path = ShapePath::new(rect.origin, 1.0, 1.0);
    path.start_at(path.absolute(0.0, body_top + corner_radius));
    path.cubic(
        true,
        [
            0.0,
            -curve_factor * corner_radius,
            curve_factor * corner_radius,
            -corner_radius,
            corner_radius,
            -corner_radius,
        ],
    );
    path.horizontal(true, rect.size.width - 2.0 * corner_radius);
    path.cubic(
        true,
        [
            curve_factor * corner_radius,
            0.0,
            corner_radius,
            curve_factor * corner_radius,
            corner_radius,
            corner_radius,
        ],
    );
    path.vertical(true, body_height - 2.0 * corner_radius);
    path.cubic(
        true,
        [
            0.0,
            curve_factor * corner_radius,
            -curve_factor * corner_radius,
            corner_radius,
            -corner_radius,
            corner_radius,
        ],
    );
    path.horizontal(true, -(rect.size.width - 2.0 * corner_radius));
    path.cubic(
        true,
        [
            -curve_factor * corner_radius,
            0.0,
            -corner_radius,
            -curve_factor * corner_radius,
            -corner_radius,
            -corner_radius,
        ],
    );
    path.close();
    path
}

fn trace_c4_person(rect: Rect, border: Point, previous: Point) -> Point {
    const HEAD_RADIUS_FACTOR: f64 = 0.22;

    let ray = extended_shape_ray(previous, border, rect);
    let head_radius = rect.size.width * HEAD_RADIUS_FACTOR;
    let head_center = Point {
        x: rect.origin.x + rect.size.width * 0.5,
        y: rect.origin.y + head_radius,
    };
    let mut intersections = shape_path_intersections(c4_person_body_path(rect), ray);
    intersections.extend(ellipse_intersections(
        head_center,
        head_radius,
        head_radius,
        ray,
    ));
    rounded_closest(border, intersections)
}

fn cloud_path(rect: Rect) -> ShapePath {
    let mut path = ShapePath::new(
        rect.origin,
        rect.size.width / 834.0,
        rect.size.height / 523.0,
    );
    path.start_at(path.absolute(137.833, 182.833));
    path.cubic(true, [0.0, 5.556, -5.556, 11.111, -11.111, 11.111]);
    path.cubic(true, [-70.833, 6.944, -126.389, 77.778, -126.389, 163.889]);
    path.cubic(true, [0.0, 91.667, 62.5, 165.278, 141.667, 165.278]);
    path.horizontal(true, 537.5);
    path.cubic(true, [84.723, 0.0, 154.167, -79.167, 154.167, -175.0]);
    path.cubic(true, [0.0, -91.667, -63.89, -168.056, -144.444, -173.611]);
    path.cubic(true, [-5.556, 0.0, -11.111, -4.167, -12.5, -11.111]);
    path.cubic(true, [-18.056, -93.055, -101.39, -162.5, -198.611, -162.5]);
    path.cubic(true, [-63.889, 0.0, -120.834, 29.167, -156.944, 75.0]);
    path.cubic(true, [-4.167, 5.556, -11.111, 6.945, -15.278, 5.556]);
    path.cubic(true, [-13.889, -5.556, -29.166, -8.333, -45.833, -8.333]);
    path.cubic(false, [196.167, 71.722, 143.389, 120.333, 137.833, 182.833]);
    path.close();
    path
}

/// Exact D2 hexagon perimeter used by the TALA release.
///
/// The right/left tips use `43.6 / 87.3`, not an exact half. `ShapePath`
/// also applies D2's float32 precision reduction to every path coordinate
/// before intersection. For odd heights this preserves a center snap point
/// that sits one pixel beyond the rounded polygon tip.
fn hexagon_path(rect: Rect) -> ShapePath {
    let mut path = ShapePath::new(rect.origin, rect.size.width, rect.size.height);
    path.start_at(path.absolute(0.25, 0.0));
    path.line(false, 0.0, 43.6 / 87.3);
    path.line(false, 0.25, 1.0);
    path.line(false, 0.75, 1.0);
    path.line(false, 1.0, 43.6 / 87.3);
    path.line(false, 0.75, 0.0);
    path.close();
    path
}

/// D2's person silhouette used by `shapePerson.TraceToShapeBorder`.
///
/// This is the release D2 `personPath` verbatim in the local path-builder
/// vocabulary.  In particular, the shoulder/head coordinates are expressed
/// in the original 68.3 × 77.4 design space; normalizing them to a unit box
/// changes the cubic intersection by a pixel for horizontal routes.
fn person_path(rect: Rect) -> ShapePath {
    let mut path = ShapePath::new(rect.origin, rect.size.width / 68.3, rect.size.height / 77.4);
    path.start_at(path.absolute(68.3, 77.4));
    path.horizontal(false, 0.0);
    path.vertical(true, -1.1);
    path.cubic(true, [0.0, -13.2, 7.5, -25.1, 19.3, -30.8]);
    path.cubic(false, [12.8, 40.9, 8.9, 33.4, 8.9, 25.2]);
    path.cubic(false, [8.9, 11.3, 20.2, 0.0, 34.1, 0.0]);
    path.cubic(true, [13.9, 0.0, 25.2, 11.3, 25.2, 25.2]);
    path.cubic(true, [0.0, 8.2, -3.8, 15.6, -10.4, 20.4]);
    path.cubic(true, [11.8, 5.7, 19.3, 17.6, 19.3, 30.8]);
    path.vertical(true, 1.0);
    path.horizontal(false, 68.3);
    path.close();
    path
}

fn trace_shape_border(shape: ShapeKind, rect: Rect, border: Point, previous: Point) -> Point {
    match shape {
        ShapeKind::Rectangle
        | ShapeKind::Square
        | ShapeKind::SqlTable
        | ShapeKind::Class
        | ShapeKind::Text
        | ShapeKind::Code
        | ShapeKind::Image => border,
        ShapeKind::Circle | ShapeKind::Oval => trace_ellipse(rect, border, previous),
        ShapeKind::Cloud => trace_shape_path(rect, border, previous, cloud_path(rect)),
        ShapeKind::Diamond => trace_shape_path(rect, border, previous, diamond_path(rect)),
        ShapeKind::Hexagon => trace_shape_path(rect, border, previous, hexagon_path(rect)),
        ShapeKind::Parallelogram => {
            let wedge = 26.0_f64.min(rect.size.width * 0.5) / rect.size.width.max(1.0);
            trace_polygon(
                rect,
                border,
                previous,
                &[(wedge, 0.0), (1.0, 0.0), (1.0 - wedge, 1.0), (0.0, 1.0)],
            )
        }
        ShapeKind::Step => {
            let wedge_width = if rect.size.width <= 35.0 {
                rect.size.width / 2.0
            } else {
                35.0
            };
            trace_pixel_polygon(
                rect,
                border,
                previous,
                &[
                    (0.0, 0.0),
                    (rect.size.width - wedge_width, 0.0),
                    (rect.size.width, rect.size.height / 2.0),
                    (rect.size.width - wedge_width, rect.size.height),
                    (0.0, rect.size.height),
                    (wedge_width, rect.size.height / 2.0),
                ],
            )
        }
        ShapeKind::Callout => trace_shape_path(rect, border, previous, callout_path(rect)),
        ShapeKind::Cylinder => trace_shape_path(rect, border, previous, cylinder_path(rect)),
        ShapeKind::Queue => trace_shape_path(rect, border, previous, queue_path(rect)),
        ShapeKind::Person => trace_shape_path(rect, border, previous, person_path(rect)),
        ShapeKind::StoredData => trace_shape_path(rect, border, previous, stored_data_path(rect)),
        ShapeKind::Page => trace_shape_path(rect, border, previous, page_outer_path(rect)),
        ShapeKind::Document => trace_shape_path(rect, border, previous, document_path(rect)),
        ShapeKind::Package => trace_shape_path(rect, border, previous, package_path(rect)),
        ShapeKind::C4Person => trace_c4_person(rect, border, previous),
    }
}

/// Recover the modifier transaction at the start of `Graph.TraceToShapeBorder`.
///
/// Multiple and 3D elements draw a second silhouette above and to the right
/// of the base shape. TALA does not enlarge the base rectangle. For a route
/// attached to the top or right face, it temporarily offsets the whole shape
/// to that outer silhouette, intersects the endpoint segment with the offset
/// face, traces the offset shape, and then restores the box.
fn modifier_trace_geometry(
    node: &ArenaNode,
    rect: Rect,
    border: Point,
    previous: Point,
) -> (Rect, Point) {
    let (dx, dy) = modifier_element_adjustments(node);
    if (dx == 0.0 && dy == 0.0)
        || border.x <= rect.origin.x + dx
        || border.y >= rect.origin.y + rect.size.height - dy
    {
        return (rect, border);
    }

    let port_side = ports(rect, node.shape)
        .into_iter()
        .find(|port| port.point == border)
        .map(|port| port.side);
    if port_side == Some(PortSide::Right) || border.x == rect.origin.x + rect.size.width {
        let translated = Rect {
            origin: Point {
                x: rect.origin.x + dx,
                y: rect.origin.y,
            },
            size: rect.size,
        };
        let top_right = Point {
            x: translated.origin.x + translated.size.width,
            y: translated.origin.y,
        };
        let bottom_right = Point {
            x: top_right.x,
            y: top_right.y + translated.size.height,
        };
        let adjusted =
            line_intersection_point(border, previous, top_right, bottom_right).unwrap_or(border);
        return (translated, adjusted);
    }
    if port_side == Some(PortSide::Top) || border.y == rect.origin.y {
        let translated = Rect {
            origin: Point {
                x: rect.origin.x,
                y: rect.origin.y - dy,
            },
            size: rect.size,
        };
        let top_right = Point {
            x: translated.origin.x + translated.size.width,
            y: translated.origin.y,
        };
        let adjusted = line_intersection_point(border, previous, translated.origin, top_right)
            .unwrap_or(border);
        return (translated, adjusted);
    }

    (rect, border)
}

/// Translation of recovered `Graph.TraceEdgesToShapeBorder` for represented
/// shape geometry.
pub(in crate::engine) fn trace_edges_to_shape_border(graph: &mut ArenaGraph) {
    let edges = (0..graph.edges.len()).collect::<Vec<_>>();
    trace_edges_to_shape_border_in(graph, &edges);
}

pub(in crate::engine) fn trace_edges_to_shape_border_in(graph: &mut ArenaGraph, edges: &[usize]) {
    for edge_index in edges.iter().copied() {
        if graph.edges[edge_index].points.len() < 2 {
            continue;
        }
        let from = graph.edges[edge_index].from;
        let to = graph.edges[edge_index].to;
        if let Some(rect) = node_rect(graph, from) {
            let border = graph.edges[edge_index].points[0];
            let previous = graph.edges[edge_index].points[1];
            let node = &graph.nodes[from.0 as usize];
            let shape = node.shape;
            let (rect, border) = modifier_trace_geometry(node, rect, border, previous);
            graph.edges[edge_index].points[0] = trace_shape_border(shape, rect, border, previous);
        }
        if let Some(rect) = node_rect(graph, to) {
            let last = graph.edges[edge_index].points.len() - 1;
            let border = graph.edges[edge_index].points[last];
            let previous = graph.edges[edge_index].points[last - 1];
            let node = &graph.nodes[to.0 as usize];
            let shape = node.shape;
            let (rect, border) = modifier_trace_geometry(node, rect, border, previous);
            graph.edges[edge_index].points[last] =
                trace_shape_border(shape, rect, border, previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Size;

    #[test]
    fn segment_intersection_rounds_displacement_before_origin() {
        let intersection = line_intersection_point(
            Point { x: 0.4, y: 0.0 },
            Point { x: 1.4, y: 1.0 },
            Point { x: 0.0, y: 0.4 },
            Point { x: 2.0, y: 0.4 },
        )
        .unwrap();

        assert_eq!(intersection, Point { x: 0.4, y: 0.0 });
    }

    #[test]
    fn step_trace_quantizes_svg_path_vertices_before_intersection() {
        let traced = trace_shape_border(
            ShapeKind::Step,
            Rect {
                origin: Point {
                    x: 2021.0,
                    y: 2446.0,
                },
                size: Size {
                    width: 116.0,
                    height: 101.0,
                },
            },
            Point {
                x: 2038.0,
                y: 2496.0,
            },
            Point {
                x: 598.0,
                y: 2496.0,
            },
        );

        assert_eq!(
            traced,
            Point {
                x: 2055.0,
                y: 2496.0
            }
        );
    }
}
