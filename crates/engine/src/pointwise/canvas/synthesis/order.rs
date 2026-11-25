// Genetica targets .NET Framework 4.0, which uses the depth-limited
// quicksort for List.Sort. Equal weights still exchange places in a
// partition. The resulting order also assigns each stamp its random sample.
pub(super) fn sort_by_key<T: Copy>(placements: &mut [T], key: impl Fn(&T) -> i32 + Copy) {
    quicksort(placements, key, 32);
}

fn quicksort<T: Copy>(mut placements: &mut [T], key: impl Fn(&T) -> i32 + Copy, mut depth: usize) {
    while placements.len() > 1 {
        if depth == 0 {
            heapsort(placements, key);
            return;
        }
        depth -= 1;
        let last = placements.len() - 1;
        let middle = last / 2;
        for (left, right) in [(0, middle), (0, last), (middle, last)] {
            if key(&placements[left]) > key(&placements[right]) {
                placements.swap(left, right);
            }
        }
        let pivot = key(&placements[middle]);
        let (mut left, mut upper) = (0, placements.len());
        loop {
            while key(&placements[left]) < pivot {
                left += 1;
            }
            while key(&placements[upper - 1]) > pivot {
                upper -= 1;
            }
            if left >= upper {
                break;
            }
            if left < upper - 1 {
                placements.swap(left, upper - 1);
            }
            left += 1;
            upper -= 1;
            if left >= upper {
                break;
            }
        }
        if upper <= placements.len() - left {
            quicksort(&mut placements[..upper], key, depth);
            placements = &mut placements[left..];
        } else {
            quicksort(&mut placements[left..], key, depth);
            placements = &mut placements[..upper];
        }
    }
}

fn heapsort<T: Copy>(placements: &mut [T], key: impl Fn(&T) -> i32 + Copy) {
    for parent in (0..placements.len() / 2).rev() {
        sift_down(placements, key, parent);
    }
    for end in (1..placements.len()).rev() {
        placements.swap(0, end);
        sift_down(&mut placements[..end], key, 0);
    }
}

fn sift_down<T: Copy>(placements: &mut [T], key: impl Fn(&T) -> i32, mut parent: usize) {
    let held = placements[parent];
    while parent < placements.len() / 2 {
        let mut child = parent * 2 + 1;
        if child + 1 < placements.len() && key(&placements[child]) < key(&placements[child + 1]) {
            child += 1;
        }
        if key(&placements[child]) < key(&held) {
            break;
        }
        placements[parent] = placements[child];
        parent = child;
    }
    placements[parent] = held;
}
