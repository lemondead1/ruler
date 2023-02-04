use glam::DVec2;

pub fn closest_point_below_line_on_circle(center: DVec2, radius: f64, start: DVec2, dir: DVec2, point: DVec2) -> DVec2 {
    let (a, b, c) = {
        let a = dir.y;
        let b = -dir.x;
        let c = start.y * dir.x - start.x * dir.y;
        let coefficient = -(a * a + b * b).sqrt() * c.signum();
        (a * coefficient, b * coefficient, c * coefficient)
    };

    let (a, b, c) = if a * center.x + b * center.y + c < 0.0 {
        (-a, -b, -c)
    } else {
        (a, b, c)
    };

    if a * point.x + b * point.y + c > 0.0 {
        point
    } else {
        match circle_intersect(center, radius, start, dir) {
            Some((p1, p2)) => {
                if p1.distance_squared(point) < p2.distance_squared(point) {
                    p1
                } else {
                    p2
                }
            }
            None => {
                println!("This shouldn't really happen");
                point
            }
        }
    }
}

fn circle_intersect(center: DVec2, radius: f64, start: DVec2, dir: DVec2) -> Option<(DVec2, DVec2)> {
    let a = dir.length_squared();
    let b = 2.0 * dir.dot(start - center);
    let c = start.distance_squared(center) - radius.powi(2);

    solve_quadratic(a, b, c).map(|solutions| (start + dir * solutions.0, start + dir * solutions.1))
}

fn solve_quadratic(a: f64, b: f64, c: f64) -> Option<(f64, f64)> {
    let d = b * b - 4.0 * a * c;
    if d < 0.0 {
        None
    } else {
        let sqrt_d = d.sqrt();
        let t1 = (-b - sqrt_d) / (2.0 * a);
        let t2 = (-b + sqrt_d) / (2.0 * a);
        Some((t1, t2))
    }
}
