use ndarray::Array1;

pub fn centered_box_to_ltrb_bulk(
    pcy: &Array1<f32>,
    pcx: &Array1<f32>,
    pw: &Array1<f32>,
    ph: &Array1<f32>,
) -> (Array1<f32>, Array1<f32>, Array1<f32>, Array1<f32>) {
    (pcx - pw * 0.5, pcy - ph * 0.5, pcx + pw * 0.5, pcy + ph * 0.5)
}

/// Detection boxes storing relavant values in place to hack SIMD
#[derive(Clone)]
pub struct DetectionBoxes {
    pub x1: Array1<f32>,
    pub y1: Array1<f32>,
    pub x2: Array1<f32>,
    pub y2: Array1<f32>,
    pub scores: Array1<f32>,
    pub classes: Array1<f32>,
    pub len: usize,
}

impl DetectionBoxes {
    pub fn new(
        x1: Array1<f32>,
        y1: Array1<f32>,
        x2: Array1<f32>,
        y2: Array1<f32>,
        scores: Array1<f32>,
        classes: Array1<f32>,
    ) -> Self {
        let len = x1.len();
        Self { x1, y1, x2, y2, scores, classes, len }
    }

    pub fn empty() -> Self {
        Self {
            x1: vec![].into(),
            y1: vec![].into(),
            x2: vec![].into(),
            y2: vec![].into(),
            scores: vec![].into(),
            classes: vec![].into(),
            len: 0,
        }
    }

    pub fn append(
        &mut self,
        x1: Array1<f32>,
        y1: Array1<f32>,
        x2: Array1<f32>,
        y2: Array1<f32>,
        scores: Array1<f32>,
        classes: Array1<f32>,
    ) {
        self.len += x1.len();
        self.x1.append(ndarray::Axis(0), x1.view()).unwrap();
        self.y1.append(ndarray::Axis(0), y1.view()).unwrap();
        self.x2.append(ndarray::Axis(0), x2.view()).unwrap();
        self.y2.append(ndarray::Axis(0), y2.view()).unwrap();
        self.scores.append(ndarray::Axis(0), scores.view()).unwrap();
        self.classes.append(ndarray::Axis(0), classes.view()).unwrap();
    }

    pub fn sort_by_score_and_trim(&mut self, len: usize) {
        let mut indices: Vec<usize> = (0..self.len).collect();
        indices.sort_unstable_by(|&a, &b| self.scores[b].partial_cmp(&self.scores[a]).unwrap());
        indices.truncate(len);
        indices.reverse();

        self.x1 = self.x1.select(ndarray::Axis(0), &indices).to_owned();
        self.y1 = self.y1.select(ndarray::Axis(0), &indices).to_owned();
        self.x2 = self.x2.select(ndarray::Axis(0), &indices).to_owned();
        self.y2 = self.y2.select(ndarray::Axis(0), &indices).to_owned();
        self.scores = self.scores.select(ndarray::Axis(0), &indices).to_owned();
        self.classes = self.classes.select(ndarray::Axis(0), &indices).to_owned();
    }
}