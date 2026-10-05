//! A small sparse logistic regression with Gaussian priors - the engine under the meta model
//! (`meta`).
//!
//! Each game is one row: `P(blue wins) = sigmoid(sum coef * beta)` over a handful of
//! parameters (side, champions, roles, pairs, players...). Priors are quadratic "links"
//! `precision / 2 * (sum a_i * beta_i - target)^2`: a link with one term pulls a parameter
//! toward a value (shrinkage), a link with two terms ties a champion's strength in one patch
//! era to the era before it (a random walk across balance changes).
//!
//! The fit is L-BFGS on the negative log posterior: every evaluation is one pass over the
//! sparse rows, nothing needs a dense matrix, and unlike single-parameter steps it does not
//! crawl when parameters move together (five team-mates who always play side by side). A warm
//! start from the previous fit converges in a few iterations. The posterior variance of each
//! parameter is approximated by the inverse of its diagonal curvature.

use std::collections::VecDeque;

#[derive(Clone, Debug, Default)]
pub struct Row {
    /// 1.0 = blue won.
    pub y: f32,
    pub weight: f32,
    pub terms: Vec<(u32, f32)>,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub precision: f32,
    pub target: f32,
    pub terms: Vec<(u32, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct Problem {
    pub params: usize,
    pub rows: Vec<Row>,
    pub links: Vec<Link>,
}

impl Problem {
    pub fn new(params: usize) -> Self {
        Self { params, rows: Vec::new(), links: Vec::new() }
    }

    /// `beta[p] ~ N(mean, sd^2)`.
    pub fn prior(&mut self, p: u32, mean: f32, sd: f32) {
        self.links.push(Link { precision: 1.0 / (sd * sd).max(1e-6), target: mean, terms: vec![(p, 1.0)] });
    }

    /// `beta[p] - beta[prev] ~ N(shift, sd^2)`.
    pub fn chain(&mut self, p: u32, prev: u32, shift: f32, sd: f32) {
        self.links.push(Link {
            precision: 1.0 / (sd * sd).max(1e-6),
            target: shift,
            terms: vec![(p, 1.0), (prev, -1.0)],
        });
    }
}

#[derive(Clone, Debug, Default)]
pub struct Fit {
    pub beta: Vec<f32>,
    /// Approximate posterior variance per parameter.
    pub var: Vec<f32>,
    /// Iterations used.
    pub sweeps: u32,
    /// Largest parameter step in the last iteration.
    pub last_change: f32,
}

pub fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

pub fn logit(p: f32) -> f32 {
    let p = p.clamp(1e-4, 1.0 - 1e-4);
    (p / (1.0 - p)).ln()
}

/// Fits the problem (L-BFGS on the negative log posterior), starting from `warm` where it has
/// values (a previous fit with the same parameter numbering, possibly shorter).
pub fn fit(problem: &Problem, warm: Option<&[f32]>, max_iterations: u32, tolerance: f32) -> Fit {
    let n = problem.params;
    let mut x = vec![0.0f64; n];
    if let Some(warm) = warm {
        for (b, w) in x.iter_mut().zip(warm) {
            if w.is_finite() {
                *b = *w as f64;
            }
        }
    }
    let mut g = vec![0.0f64; n];
    let mut f = objective(problem, &x, &mut g);
    const MEMORY: usize = 8;
    let mut history: VecDeque<(Vec<f64>, Vec<f64>, f64)> = VecDeque::new();
    let mut iterations = 0;
    let mut last_change = f32::INFINITY;
    let mut x_new = vec![0.0f64; n];
    let mut g_new = vec![0.0f64; n];
    while iterations < max_iterations {
        let gmax = g.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        if gmax < tolerance as f64 {
            last_change = 0.0;
            break;
        }
        iterations += 1;
        // two-loop recursion: d = -H g
        let mut d: Vec<f64> = g.iter().map(|v| -v).collect();
        let mut alphas = Vec::with_capacity(history.len());
        for (s, y, rho) in history.iter().rev() {
            let a = rho * dot(s, &d);
            axpy(-a, y, &mut d);
            alphas.push(a);
        }
        let gamma = match history.back() {
            Some((s, y, _)) => dot(s, y) / dot(y, y).max(1e-12),
            None => 1.0 / gmax.max(1.0),
        };
        d.iter_mut().for_each(|v| *v *= gamma);
        for ((s, y, rho), a) in history.iter().zip(alphas.iter().rev()) {
            let b = rho * dot(y, &d);
            axpy(a - b, s, &mut d);
        }
        let mut slope = dot(&g, &d);
        if slope >= 0.0 {
            // not a descent direction: start the memory over
            history.clear();
            d = g.iter().map(|v| -v / gmax.max(1.0)).collect();
            slope = dot(&g, &d);
        }
        // backtracking line search (Armijo)
        let mut step = 1.0f64;
        let mut accepted = false;
        let mut converged = false;
        for _ in 0..30 {
            for i in 0..n {
                x_new[i] = x[i] + step * d[i];
            }
            let f_new = objective(problem, &x_new, &mut g_new);
            if f_new.is_finite() && f_new <= f + 1e-4 * step * slope {
                accepted = true;
                let s_vec: Vec<f64> = (0..n).map(|i| x_new[i] - x[i]).collect();
                let y_vec: Vec<f64> = (0..n).map(|i| g_new[i] - g[i]).collect();
                let sy = dot(&s_vec, &y_vec);
                last_change = s_vec.iter().fold(0.0f64, |m, v| m.max(v.abs())) as f32;
                if sy > 1e-12 {
                    if history.len() == MEMORY {
                        history.pop_front();
                    }
                    history.push_back((s_vec, y_vec, 1.0 / sy));
                }
                std::mem::swap(&mut x, &mut x_new);
                std::mem::swap(&mut g, &mut g_new);
                converged = (f - f_new).abs() < 1e-11 * (1.0 + f_new.abs());
                f = f_new;
                break;
            }
            step *= 0.5;
        }
        if !accepted || converged {
            break;
        }
    }
    let var = curvature(problem, &x).iter().map(|h| if *h > 0.0 { (1.0 / h) as f32 } else { 0.0 }).collect();
    Fit { beta: x.iter().map(|v| *v as f32).collect(), var, sweeps: iterations, last_change }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn axpy(a: f64, x: &[f64], y: &mut [f64]) {
    for (yi, xi) in y.iter_mut().zip(x) {
        *yi += a * xi;
    }
}

fn eta(terms: &[(u32, f32)], x: &[f64]) -> f64 {
    terms.iter().map(|&(p, a)| a as f64 * x.get(p as usize).copied().unwrap_or(0.0)).sum()
}

/// Negative log posterior and its gradient.
fn objective(problem: &Problem, x: &[f64], grad: &mut [f64]) -> f64 {
    grad.iter_mut().for_each(|v| *v = 0.0);
    let mut f = 0.0f64;
    for row in &problem.rows {
        let e = eta(&row.terms, x);
        let w = row.weight as f64;
        let y = row.y as f64;
        // log(1 + e^e), computed stably
        let softplus = if e > 0.0 { e + (-e).exp().ln_1p() } else { e.exp().ln_1p() };
        f += w * (softplus - y * e);
        let r = w * (1.0 / (1.0 + (-e).exp()) - y);
        for &(p, a) in &row.terms {
            if let Some(gp) = grad.get_mut(p as usize) {
                *gp += r * a as f64;
            }
        }
    }
    for link in &problem.links {
        let lam = link.precision as f64;
        let r = eta(&link.terms, x) - link.target as f64;
        f += 0.5 * lam * r * r;
        for &(p, a) in &link.terms {
            if let Some(gp) = grad.get_mut(p as usize) {
                *gp += lam * r * a as f64;
            }
        }
    }
    f
}

/// The diagonal of the Hessian at `x`.
fn curvature(problem: &Problem, x: &[f64]) -> Vec<f64> {
    let mut h = vec![0.0f64; problem.params];
    for row in &problem.rows {
        let mu = 1.0 / (1.0 + (-eta(&row.terms, x)).exp());
        let k = row.weight as f64 * (mu * (1.0 - mu)).max(1e-6);
        for &(p, a) in &row.terms {
            if let Some(hp) = h.get_mut(p as usize) {
                *hp += k * (a as f64) * (a as f64);
            }
        }
    }
    for link in &problem.links {
        for &(p, a) in &link.terms {
            if let Some(hp) = h.get_mut(p as usize) {
                *hp += link.precision as f64 * (a as f64) * (a as f64);
            }
        }
    }
    h
}

/// The model's blue-win probability for one row.
pub fn predict(terms: &[(u32, f32)], beta: &[f32]) -> f32 {
    sigmoid(terms.iter().map(|&(p, a)| a * beta.get(p as usize).copied().unwrap_or(0.0)).sum())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic pseudo-random numbers in [0, 1).
    pub struct Lcg(pub u64);
    impl Lcg {
        pub fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32
        }
    }

    #[test]
    fn recovers_strengths_from_simulated_games() {
        // 12 champions with known strengths; random 5v5 games
        let truth: Vec<f32> = (0..12).map(|i| (i as f32 - 5.5) * 0.08).collect();
        let mut rng = Lcg(3);
        let mut problem = Problem::new(12);
        for p in 0..12 {
            problem.prior(p, 0.0, 1.0);
        }
        for _ in 0..6000 {
            let mut ids: Vec<u32> = (0..12).collect();
            for i in (1..ids.len()).rev() {
                let j = (rng.next() * (i + 1) as f32) as usize;
                ids.swap(i, j.min(i));
            }
            let mut terms = Vec::new();
            let mut eta = 0.0;
            for (k, id) in ids.iter().take(10).enumerate() {
                let sign = if k < 5 { 1.0 } else { -1.0 };
                terms.push((*id, sign));
                eta += sign * truth[*id as usize];
            }
            let y = if rng.next() < sigmoid(eta) { 1.0 } else { 0.0 };
            problem.rows.push(Row { y, weight: 1.0, terms });
        }
        let fit = fit(&problem, None, 200, 1e-5);
        // strengths are only identified up to a common shift: compare centred values
        let mean: f32 = fit.beta.iter().sum::<f32>() / 12.0;
        for (b, t) in fit.beta.iter().zip(&truth) {
            assert!((b - mean - t).abs() < 0.08, "{b} vs {t}");
        }
        assert!(fit.var.iter().all(|v| *v > 0.0 && *v < 0.01));
        // warm start: already converged
        let again = super::fit(&problem, Some(&fit.beta), 200, 1e-4);
        assert!(again.sweeps <= 3, "{}", again.sweeps);
    }

    #[test]
    fn chains_carry_strength_across_eras() {
        // param 0 = era 1, param 1 = era 2 (after a buff of +0.2 expected); no games in era 2
        let mut problem = Problem::new(2);
        problem.prior(0, 0.0, 1.0);
        problem.chain(1, 0, 0.2, 0.1);
        for i in 0..400 {
            // era 1: wins 60% against an implicit average opponent
            problem.rows.push(Row { y: if i % 5 < 3 { 1.0 } else { 0.0 }, weight: 1.0, terms: vec![(0, 1.0)] });
        }
        let fit = fit(&problem, None, 500, 1e-6);
        assert!((sigmoid(fit.beta[0]) - 0.6).abs() < 0.02, "{}", sigmoid(fit.beta[0]));
        assert!((fit.beta[1] - fit.beta[0] - 0.2).abs() < 1e-3, "carried with the buff");
        assert!(fit.var[1] > fit.var[0], "less sure about the new era");
    }

    #[test]
    fn priors_hold_unseen_parameters() {
        let mut problem = Problem::new(3);
        problem.prior(2, 0.5, 0.3);
        let fit = fit(&problem, None, 50, 1e-6);
        assert_eq!(fit.beta[0], 0.0);
        assert!((fit.beta[2] - 0.5).abs() < 1e-5);
        assert!((fit.var[2] - 0.09).abs() < 1e-4);
        assert_eq!(predict(&[(2, 1.0)], &fit.beta), sigmoid(0.5));
    }
}
