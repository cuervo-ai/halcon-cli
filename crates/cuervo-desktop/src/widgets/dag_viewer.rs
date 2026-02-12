use egui::{Pos2, Rect, RichText, Ui, Vec2};

use crate::theme::CuervoTheme;

/// A node in the DAG visualization.
pub struct DagNode {
    pub id: String,
    pub label: String,
    pub status: String,
    pub wave: usize,
    pub depends_on: Vec<String>,
}

/// Render a DAG visualization from a list of nodes.
pub fn render_dag(ui: &mut Ui, nodes: &[DagNode]) {
    if nodes.is_empty() {
        ui.label(RichText::new("Empty DAG").color(CuervoTheme::TEXT_MUTED));
        return;
    }

    let max_wave = nodes.iter().map(|n| n.wave).max().unwrap_or(0);
    let node_width = 120.0;
    let node_height = 40.0;
    let h_spacing = 160.0;
    let v_spacing = 60.0;

    let total_width = (max_wave + 1) as f32 * h_spacing;
    let max_nodes_in_wave = (0..=max_wave)
        .map(|w| nodes.iter().filter(|n| n.wave == w).count())
        .max()
        .unwrap_or(1);
    let total_height = max_nodes_in_wave as f32 * v_spacing;

    let (response, painter) =
        ui.allocate_painter(Vec2::new(total_width + 40.0, total_height + 40.0), egui::Sense::hover());
    let origin = response.rect.min + Vec2::new(20.0, 20.0);

    // Compute positions.
    let mut positions: std::collections::HashMap<String, Pos2> = std::collections::HashMap::new();
    for wave in 0..=max_wave {
        let wave_nodes: Vec<_> = nodes.iter().filter(|n| n.wave == wave).collect();
        for (i, node) in wave_nodes.iter().enumerate() {
            let x = origin.x + wave as f32 * h_spacing;
            let y = origin.y + i as f32 * v_spacing;
            positions.insert(node.id.clone(), Pos2::new(x, y));
        }
    }

    // Draw edges.
    for node in nodes {
        if let Some(&to_pos) = positions.get(&node.id) {
            let to_center = to_pos + Vec2::new(0.0, node_height / 2.0);
            for dep_id in &node.depends_on {
                if let Some(&from_pos) = positions.get(dep_id) {
                    let from_center = from_pos + Vec2::new(node_width, node_height / 2.0);
                    painter.line_segment(
                        [from_center, to_center],
                        egui::Stroke::new(1.5, CuervoTheme::BORDER),
                    );
                }
            }
        }
    }

    // Draw nodes.
    for node in nodes {
        if let Some(&pos) = positions.get(&node.id) {
            let rect = Rect::from_min_size(pos, Vec2::new(node_width, node_height));
            let color = CuervoTheme::task_status_color(&node.status);
            let bg = color.linear_multiply(0.2);

            painter.rect_filled(rect, 6.0, bg);
            painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.0, color));

            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &node.label,
                egui::FontId::monospace(11.0),
                CuervoTheme::TEXT_PRIMARY,
            );
        }
    }
}

/// Build DagNode list from task execution data.
pub fn nodes_from_task(task: &cuervo_api::types::task::TaskExecution) -> Vec<DagNode> {
    task.node_results
        .iter()
        .enumerate()
        .map(|(i, nr)| DagNode {
            id: nr.task_id.to_string(),
            label: nr.task_id.to_string()[..8].to_string(),
            status: format!("{:?}", nr.status).to_lowercase(),
            wave: i, // Approximate wave — real wave info from DAG topology.
            depends_on: vec![],
        })
        .collect()
}
