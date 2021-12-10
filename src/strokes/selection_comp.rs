use crate::pens::selector::Selector;
use crate::strokes::render_comp::RenderComponent;
use crate::strokes::trash_comp::TrashComponent;
use crate::{compose, geometry};

use super::{StrokeKey, StrokeStyle, StrokesState};
use crate::strokes::strokestyle::StrokeBehaviour;

use gtk4::{gio, prelude::*};
use p2d::bounding_volume::BoundingVolume;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SelectionComponent {
    pub selected: bool,
}

impl Default for SelectionComponent {
    fn default() -> Self {
        Self { selected: false }
    }
}

impl SelectionComponent {
    pub fn new(selected: bool) -> Self {
        Self { selected }
    }
}

impl StrokesState {
    /// Returns false if selecting is unsupported
    pub fn can_select(&self, key: StrokeKey) -> bool {
        self.selection_components.get(key).is_some()
    }

    pub fn selected(&self, key: StrokeKey) -> Option<bool> {
        if let Some(selection_comp) = self.selection_components.get(key) {
            Some(selection_comp.selected)
        } else {
            log::warn!(
                "failed to get selection_component for stroke with key {:?}, invalid key used or stroke does not support selecting",
                key
            );
            None
        }
    }

    /// N
    pub fn set_selected(&mut self, key: StrokeKey, selected: bool) {
        if let Some(selection_comp) = self.selection_components.get_mut(key) {
            selection_comp.selected = selected;
            self.update_selection_bounds();
        } else {
            log::warn!(
                "failed to get selection_component for stroke with key {:?}, invalid key used or stroke does not support selecting",
                key
            );
        }
    }

    pub fn selection_keys(&self) -> Vec<StrokeKey> {
        self.selection_components
            .iter()
            .par_bridge()
            .filter_map(|(key, selection_comp)| {
                if selection_comp.selected {
                    Some(key)
                } else {
                    None
                }
            })
            .collect::<Vec<StrokeKey>>()
    }

    pub fn last_selection_key(&self) -> Option<StrokeKey> {
        let chrono_components = &self.chrono_components;
        let trash_components = &self.trash_components;
        let selection_components = &self.selection_components;

        let mut sorted: Vec<(StrokeKey, u32)> = chrono_components
            .iter()
            .par_bridge()
            .filter_map(|(key, chrono_comp)| {
                if let (Some(trash_comp), Some(selection_comp)) =
                    (trash_components.get(key), selection_components.get(key))
                {
                    if !trash_comp.trashed && selection_comp.selected {
                        return Some((key, chrono_comp.t));
                    }
                }
                None
            })
            .collect();
        sorted.sort_unstable_by(|first, second| first.1.cmp(&second.1));

        let last_selection_key = sorted.last().copied();
        if let Some(last_stroke_key) = last_selection_key {
            Some(last_stroke_key.0)
        } else {
            None
        }
    }

    pub fn selection_len(&self) -> usize {
        self.selection_keys().len()
    }

    pub fn update_selection_bounds(&mut self) {
        self.selection_bounds = self.gen_bounds(self.selection_keys().iter());
    }

    pub fn deselect(&mut self) {
        self.selection_components
            .iter_mut()
            .for_each(|(_key, selection_comp)| selection_comp.selected = false);

        self.selection_bounds = None;
    }

    pub fn duplicate_selection(&mut self) {
        let offset = na::vector![20.0, 20.0];

        self.selection_components
            .iter_mut()
            .filter_map(|(key, selection_comp)| {
                if selection_comp.selected {
                    selection_comp.selected = false;
                    let dup_key = self.strokes.insert(self.strokes.get(key).unwrap().clone());

                    Some(dup_key)
                } else {
                    None
                }
            })
            .collect::<Vec<StrokeKey>>()
            // Need to collect to avoid borrow errors
            .iter()
            .for_each(|dup_key| {
                self.selection_components
                    .insert(*dup_key, SelectionComponent::new(true));
                self.render_components
                    .insert(*dup_key, RenderComponent::default());
                self.trash_components
                    .insert(*dup_key, TrashComponent::default());

                // Offsetting the new selection to make the duplication apparent to the user
                if let Some(stroke) = self.strokes.get_mut(*dup_key) {
                    stroke.translate(offset);
                }
                self.update_rendering_for_stroke(*dup_key);
            });

        self.update_selection_bounds();
    }

    /// Returns true if selection has changed
    pub fn update_selection_for_selector(
        &mut self,
        selector: &Selector,
        viewport: Option<p2d::bounding_volume::AABB>,
    ) -> bool {
        let selection_len_prev = self.selection_len();
        let selector_bounds = if let Some(selector_bounds) = selector.bounds {
            selector_bounds
        } else {
            return false;
        };

        self.strokes.iter().for_each(|(key, stroke)| {
            // Skip if stroke is hidden
            if let (Some(render_comp), Some(trash_comp)) = (
                self.render_components.get(key),
                self.trash_components.get(key),
            ) {
                if !render_comp.render || trash_comp.trashed {
                    return;
                }
            }
            // skip if stroke is not in viewport
            if let Some(viewport) = viewport {
                if !viewport.intersects(&stroke.bounds()) {
                    return;
                }
            }
            if let Some(selection_comp) = self.selection_components.get_mut(key) {
                // Default to not selected, check if selected
                selection_comp.selected = false;

                match stroke {
                    StrokeStyle::MarkerStroke(markerstroke) => {
                        if selector_bounds.contains(&markerstroke.bounds) {
                            selection_comp.selected = true;
                        } else if selector_bounds.intersects(&markerstroke.bounds) {
                            for hitbox_elem in markerstroke.hitbox.iter() {
                                if !selector_bounds.contains(hitbox_elem) {
                                    return;
                                }
                            }
                            selection_comp.selected = true;
                        }
                    }
                    StrokeStyle::BrushStroke(brushstroke) => {
                        if selector_bounds.contains(&brushstroke.bounds) {
                            selection_comp.selected = true;
                        } else if selector_bounds.intersects(&brushstroke.bounds) {
                            for hitbox_elem in brushstroke.hitbox.iter() {
                                if !selector_bounds.contains(hitbox_elem) {
                                    return;
                                }
                            }
                            selection_comp.selected = true;
                        }
                    }
                    StrokeStyle::ShapeStroke(shapestroke) => {
                        if selector_bounds.contains(&shapestroke.bounds) {
                            selection_comp.selected = true;
                        }
                    }
                    StrokeStyle::VectorImage(vector_image) => {
                        if selector_bounds.contains(&vector_image.bounds) {
                            selection_comp.selected = true;
                        }
                    }
                    StrokeStyle::BitmapImage(vector_image) => {
                        if selector_bounds.contains(&vector_image.bounds) {
                            selection_comp.selected = true;
                        }
                    }
                }
            }
        });

        if self.selection_len() != selection_len_prev {
            self.update_selection_bounds();
            self.update_rendering_for_selection();
            true
        } else {
            false
        }
    }

    /// Resizing the selection with its contents to the new bounds
    pub fn resize_selection(&mut self, new_bounds: p2d::bounding_volume::AABB) {
        fn calc_new_stroke_bounds(
            stroke: &StrokeStyle,
            selection_bounds: p2d::bounding_volume::AABB,
            new_bounds: p2d::bounding_volume::AABB,
        ) -> p2d::bounding_volume::AABB {
            let offset = na::vector![
                new_bounds.mins[0] - selection_bounds.mins[0],
                new_bounds.mins[1] - selection_bounds.mins[1]
            ];

            let scalevector = na::vector![
                (new_bounds.maxs[0] - new_bounds.mins[0])
                    / (selection_bounds.maxs[0] - selection_bounds.mins[0]),
                (new_bounds.maxs[1] - new_bounds.mins[1])
                    / (selection_bounds.maxs[1] - selection_bounds.mins[1])
            ];

            p2d::bounding_volume::AABB::new(
                na::point![
                    (stroke.bounds().mins[0] - selection_bounds.mins[0]) * scalevector[0]
                        + selection_bounds.mins[0]
                        + offset[0],
                    (stroke.bounds().mins[1] - selection_bounds.mins[1]) * scalevector[1]
                        + selection_bounds.mins[1]
                        + offset[1]
                ],
                na::point![
                    (stroke.bounds().mins[0] - selection_bounds.mins[0]) * scalevector[0]
                        + selection_bounds.mins[0]
                        + offset[0]
                        + (stroke.bounds().maxs[0] - stroke.bounds().mins[0]) * scalevector[0],
                    (stroke.bounds().mins[1] - selection_bounds.mins[1]) * scalevector[1]
                        + selection_bounds.mins[1]
                        + offset[1]
                        + (stroke.bounds().maxs[1] - stroke.bounds().mins[1]) * scalevector[1]
                ],
            )
        }

        if let Some(selection_bounds) = self.selection_bounds {
            self.strokes.iter_mut().for_each(|(key, stroke)| {
                if let Some(selection_comp) = self.selection_components.get(key) {
                    if selection_comp.selected {
                        stroke.resize(calc_new_stroke_bounds(stroke, selection_bounds, new_bounds));
                    }
                }
            });

            self.selection_bounds = Some(new_bounds);
            self.update_rendering_for_selection();
        }
    }

    /// Translate the selection with its contents with an offset relative to the current position
    pub fn translate_selection(&mut self, offset: na::Vector2<f64>) {
        self.strokes.iter_mut().for_each(|(key, stroke)| {
            if let Some(selection_comp) = self.selection_components.get(key) {
                if selection_comp.selected {
                    stroke.translate(offset);
                }
            }
        });

        self.selection_bounds = if let Some(bounds) = self.selection_bounds {
            Some(geometry::aabb_translate(bounds, offset))
        } else {
            None
        };
        self.update_rendering_for_selection();
    }

    pub fn gen_svg_from_strokes(&self) -> Result<String, anyhow::Error> {
        let mut data = String::new();

        self.render_components
            .iter()
            .filter_map(|(key, render_comp)| {
                if render_comp.render && !self.trashed(key).unwrap_or_else(|| true) {
                    Some(key)
                } else {
                    None
                }
            })
            .for_each(|key| {
                if let Some(stroke) = self.strokes.get(key) {
                    match stroke.gen_svg_data(na::vector![0.0, 0.0]) {
                        Ok(data_entry) => {
                            data.push_str(&data_entry.as_str());
                        }
                        Err(e) => {
                            log::error!("gen_svg_data() failed for stroke with key {:?} in gen_svg_from_strokes(), {}", key, e);
                        }
                    }
                }
            });

        Ok(data)
    }

    pub fn export_selection_as_svg(&self, file: gio::File) -> Result<(), anyhow::Error> {
        if let Some(selection_bounds) = self.selection_bounds {
            let mut data = self
                .selection_keys()
                .iter()
                .filter_map(|key| self.strokes.get(*key))
                .filter_map(|stroke| {
                    stroke
                        .gen_svg_data(na::vector![
                            -selection_bounds.mins[0],
                            -selection_bounds.mins[1]
                        ])
                        .ok()
                })
                .fold(String::from(""), |acc, x| acc + x.as_str() + "\n");

            let wrapper_bounds = p2d::bounding_volume::AABB::new(
                na::point![0.0, 0.0],
                na::point![
                    selection_bounds.maxs[0] - selection_bounds.mins[0],
                    selection_bounds.maxs[1] - selection_bounds.mins[1]
                ],
            );
            data = compose::wrap_svg(
                data.as_str(),
                Some(wrapper_bounds),
                Some(wrapper_bounds),
                true,
                false,
            );

            let output_stream = file.replace::<gio::Cancellable>(
                None,
                false,
                gio::FileCreateFlags::REPLACE_DESTINATION,
                None,
            )?;
            output_stream.write::<gio::Cancellable>(data.as_bytes(), None)?;
            output_stream.close::<gio::Cancellable>(None)?;
        }

        Ok(())
    }
}