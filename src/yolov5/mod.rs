pub mod utils;
use std::fmt;

use itertools::{izip, Itertools};
use ndarray::{Array1, Array3};
use numpy::{PyReadonlyArray3, PyReadonlyArray5};
use pyo3::prelude::*;
use utils::{centered_box_to_ltrb_bulk, DetectionBoxes};

use crate::common::ssd_postprocess::{BoundingBox, DetectionResult, DetectionResults};
use crate::common::PyDetectionResults;

#[derive(Debug, Clone)]
pub struct RustPostprocessor {
    pub anchors: Array3<f32>,
    pub strides: Vec<f32>,
}

impl fmt::Display for RustPostprocessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shape = self.anchors.shape();
        write!(
            f,
            "RustPostProcessor {{ num_detection_layers: {}, num_anchor: {}, strides: {:?} }}",
            shape[0], shape[1], self.strides
        )
    }
}

impl RustPostprocessor {
    fn new(anchors: Array3<f32>, strides: Vec<f32>) -> Self {
        pub const NUM_ANCHOR_LAST: usize = 2;
        assert_eq!(
            anchors.shape()[2],
            NUM_ANCHOR_LAST,
            "anchors' last dimension must be {NUM_ANCHOR_LAST}"
        );
        Self { anchors, strides }
    }

    fn box_decode(
        &self,
        inputs: Vec<PyReadonlyArray5<'_, f32>>,
        conf_threshold: f32,
    ) -> Vec<DetectionBoxes> {
        const MAX_BOXES: usize = 10_000;
        let mut num_rows: usize = 0;

        let batch_size = inputs[0].shape()[0];
        let mut detection_boxes: Vec<DetectionBoxes> = vec![DetectionBoxes::empty(); batch_size];

        'outer: for (&stride, anchors_inner_stride, inner_stride) in
            izip!(&self.strides, self.anchors.outer_iter(), inputs)
        {
            for (batch_index, inner_batch) in inner_stride.as_array().outer_iter().enumerate() {
                // Perform box_decode for one batch
                let mut pcy: Vec<f32> = Vec::with_capacity(MAX_BOXES);
                let mut pcx: Vec<f32> = Vec::with_capacity(MAX_BOXES);
                let mut ph: Vec<f32> = Vec::with_capacity(MAX_BOXES);
                let mut pw: Vec<f32> = Vec::with_capacity(MAX_BOXES);

                let mut scores: Vec<f32> = Vec::with_capacity(MAX_BOXES);
                let mut classes: Vec<usize> = Vec::with_capacity(MAX_BOXES);
                for (anchors, inner_anchor) in
                    izip!(anchors_inner_stride.outer_iter(), inner_batch.outer_iter())
                {
                    let &[ax, ay] = (anchors.to_owned() * stride).as_slice().unwrap() else {
                        unreachable!()
                    };
                    for (y, inner_y) in inner_anchor.outer_iter().enumerate() {
                        for (x, inner_x) in inner_y.outer_iter().enumerate() {
                            // Destruct output array
                            let &[bx, by, bw, bh, object_confidence, ref class_confs @ ..] =
                                inner_x.as_slice().expect("inner_x must be contiguous")
                            else {
                                unreachable!()
                            };

                            // Low object confidence, skip
                            if object_confidence <= conf_threshold {
                                continue;
                            };
                            let candidates = (0..class_confs.len())
                                .filter(|&i| unsafe {class_confs.get_unchecked(i)} * object_confidence > conf_threshold)
                                .collect_vec();

                            // (feat[..., 0:2] * 2. - 0.5 + self.grid[i]) * self.stride[i]  # xy
                            // (feat[..., 2:4] * 2) ** 2 * self.anchor_grid[i]  # wh
                            // yolov5 boundingbox format(center_x,center_y,width,height)
                            let cy = (by * 2.0 - 0.5 + y as f32) * stride;
                            let cx = (bx * 2.0 - 0.5 + x as f32) * stride;
                            let h = 4.0 * bh * bh * ay;
                            let w = 4.0 * bw * bw * ax;

                            for c in candidates {
                                pcy.push(cy);
                                pcx.push(cx);
                                ph.push(h);
                                pw.push(w);
                                scores.push(
                                    unsafe { class_confs.get_unchecked(c) } * object_confidence,
                                );
                                classes.push(c);

                                num_rows += 1;
                                if num_rows >= MAX_BOXES {
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                // Convert centered boxes to LTRB boxes at once
                let (x1, y1, x2, y2): (Array1<f32>, Array1<f32>, Array1<f32>, Array1<f32>) =
                    centered_box_to_ltrb_bulk(&pcy.into(), &pcx.into(), &pw.into(), &ph.into());
                detection_boxes[batch_index].append(x1, y1, x2, y2, scores.into(), classes.into());
            }
        }

        detection_boxes
    }

    /// Non-Maximum Suppression Algorithm
    /// Faster implementation by Malisiewicz et al.
    fn nms(
        boxes: &DetectionBoxes,
        iou_threshold: f32,
        epsilon: Option<f32>,
        agnostic: Option<bool>,
    ) -> Vec<usize> {
        const MAX_BOXES: usize = 300;
        const MAX_WH: f32 = 7680.;
        let agnostic = agnostic.unwrap_or(false);
        let epsilon = epsilon.unwrap_or(1e-5);

        let c = if agnostic {
            Array1::zeros(boxes.len)
        } else {
            boxes.classes.mapv(|v| v as f32) * MAX_WH
        };
        let x1 = &boxes.x1 + &c;
        let y1 = &boxes.y1 + &c;
        let x2 = &boxes.x2 + &c;
        let y2 = &boxes.y2 + &c;

        let mut indices: Vec<usize> = (0..boxes.len).collect();
        let mut results: Vec<usize> = Vec::new();

        let dx = (&x2 - &x1).map(|&v| f32::max(0., v));
        let dy = (&y2 - &y1).map(|&v| f32::max(0., v));
        let areas: Array1<f32> = dx * dy;

        // Performs unstable argmax `indices = argmax(boxes.scores)`
        indices.sort_unstable_by(|&i, &j| {
            let box_score_i = unsafe { boxes.scores.uget(i) };
            let box_score_j = unsafe { boxes.scores.uget(j) };
            box_score_i.partial_cmp(box_score_j).unwrap()
        });

        while let Some(cur_idx) = indices.pop() {
            if results.len() > MAX_BOXES {
                break;
            }
            results.push(cur_idx);

            let xx1: Array1<f32> = indices
                .iter()
                .map(|&i| unsafe { f32::max(*x1.uget(cur_idx), *x1.uget(i)) })
                .collect();
            let yy1: Array1<f32> = indices
                .iter()
                .map(|&i| unsafe { f32::max(*y1.uget(cur_idx), *y1.uget(i)) })
                .collect();
            let xx2: Array1<f32> = indices
                .iter()
                .map(|&i| unsafe { f32::min(*x2.uget(cur_idx), *x2.uget(i)) })
                .collect();
            let yy2: Array1<f32> = indices
                .iter()
                .map(|&i| unsafe { f32::min(*y2.uget(cur_idx), *y2.uget(i)) })
                .collect();

            let widths = (xx2 - xx1).mapv(|v| f32::max(0.0, v));
            let heights = (yy2 - yy1).mapv(|v| f32::max(0.0, v));

            let ious = widths * heights;
            let cut_areas: Array1<f32> =
                indices.iter().map(|&i| unsafe { *areas.uget(i) }).collect();
            let overlap = &ious / (unsafe { *areas.uget(cur_idx) } + cut_areas - &ious + epsilon);

            indices = indices
                .into_iter()
                .enumerate()
                .filter_map(|(i, j)| (unsafe { *overlap.uget(i) } <= iou_threshold).then_some(j))
                .collect();
        }

        results
    }

    /// YOLOv5 postprocess function
    /// The vector in function input/output is for batched input/output
    fn postprocess(
        &self,
        inputs: Vec<PyReadonlyArray5<'_, f32>>,
        conf_threshold: f32,
        iou_threshold: f32,
        epsilon: Option<f32>,
        agnostic: Option<bool>,
    ) -> Vec<DetectionResults> {
        let max_nms: usize = 30_000;
        let mut detection_boxes = self.box_decode(inputs, conf_threshold);
        // Inner vector for the result indexes in one image, outer vector for batch
        let indices: Vec<Vec<usize>> = detection_boxes
            .iter_mut()
            .map(|dbox| {
                if dbox.len > max_nms {
                    dbox.sort_by_score_and_trim(max_nms);
                };
                Self::nms(dbox, iou_threshold, epsilon, agnostic)
            })
            .collect();

        izip!(detection_boxes, indices)
            .map(|(dbox, indexes)| {
                DetectionResults(
                    indexes
                        .into_iter()
                        .map(|i| {
                            DetectionResult::new_detection_result(
                                i as f32,
                                BoundingBox::new_bounding_box(
                                    dbox.y1[i], dbox.x1[i], dbox.y2[i], dbox.x2[i],
                                ),
                                dbox.scores[i],
                                dbox.classes[i] as f32,
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }
}

/// YOLOv5 PostProcessor
///
/// It takes anchors, class_names, strides as input
///
/// Args:
///     anchors (numpy.ndarray): Anchors (3D Array)
///     strides (numpy.ndarray): Strides (1D Array)
#[pyclass]
pub struct RustPostProcessor(RustPostprocessor);

#[pymethods]
impl RustPostProcessor {
    #[new]
    fn new(anchors: PyReadonlyArray3<'_, f32>, strides: Vec<f32>) -> PyResult<Self> {
        Ok(Self(RustPostprocessor::new(anchors.to_owned_array(), strides)))
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("{:?}", self.0))
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!("{}", self.0))
    }

    /// Evaluate the postprocess
    ///
    /// Args:
    ///     inputs (Sequence[numpy.ndarray]): Input tensors
    ///     conf_threshold (float): Confidence threshold
    ///     iou_threshold (float): IoU threshold
    ///     epsilon (Optional[float]): Epsilon for numerical stability
    ///     agnostic (Optional[bool]): Whether to use agnostic NMS
    ///
    /// Returns:
    ///     List[numpy.ndarray]: Batched detection results
    fn eval(
        &self,
        inputs: Vec<PyReadonlyArray5<'_, f32>>,
        conf_threshold: f32,
        iou_threshold: f32,
        epsilon: Option<f32>,
        agnostic: Option<bool>,
    ) -> PyResult<Vec<PyDetectionResults>> {
        Ok(self
            .0
            .postprocess(inputs, conf_threshold, iou_threshold, epsilon, agnostic)
            .into_iter()
            .map(PyDetectionResults::from)
            .collect())
    }
}

pub(crate) fn yolov5(m: &PyModule) -> PyResult<()> {
    m.add_class::<RustPostProcessor>()?;

    Ok(())
}
