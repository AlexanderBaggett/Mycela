//! Explicit default nz scheduling boundaries for independent numeric workers.
//! Thread.Abort, affinity and concurrent error races remain unproved.
use crate::ImageError;

/// Explicit source scheduler context. Regular z5 callers use WorkerPool with
/// their actual processor count. CallingThread names the source aaa bypass.
/// Dense kernels can use scoped workers for large images. The source chunk
/// iterators below dispatch their ranges sequentially.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProceduralExecution {
    CallingThread,
    WorkerPool { processor_count: i32 },
}

pub(crate) fn chunks<E>(
    total: i32,
    execution: ProceduralExecution,
) -> Result<Vec<(i32, i32)>, crate::PointwiseError<E>> {
    chunks_with_minimum(total, execution, 50)
}

pub(crate) fn row_chunks<E>(
    total: i32,
    execution: ProceduralExecution,
) -> Result<Vec<(i32, i32)>, crate::PointwiseError<E>> {
    chunks_with_minimum(total, execution, 1)
}
/// Actual q9.b row workers select minimum1; existing callers retain minimum50.
fn chunks_with_minimum<E>(
    total: i32,
    execution: ProceduralExecution,
    minimum: i32,
) -> Result<Vec<(i32, i32)>, crate::PointwiseError<E>> {
    let cores = match execution {
        ProceduralExecution::CallingThread => return Ok(vec![(0, total)]),
        ProceduralExecution::WorkerPool { processor_count } => processor_count,
    };
    if total == 0 {
        return Ok(Vec::new());
    }
    if cores <= 0 {
        return Err(crate::PointwiseError::Argument);
    }
    let divisor = cores.wrapping_mul(2);
    if divisor == 0 {
        return Err(crate::PointwiseError::DivideByZero);
    }
    let mut chunk = (total / divisor).max(minimum);
    if total <= minimum {
        return Ok(vec![(0, total)]);
    }
    let mut result = Vec::new();
    let mut start = 0i32;
    while start < total {
        if chunk > minimum && chunk.wrapping_mul(cores) >= total.wrapping_sub(start) {
            chunk /= 2;
        }
        let count = if start.wrapping_add(chunk) <= total {
            chunk
        } else {
            total.wrapping_sub(start)
        };
        result.try_reserve(1).map_err(|_| ImageError::Allocation)?;
        result.push((start, count));
        start = start.wrapping_add(chunk);
    }
    Ok(result)
}

pub(crate) fn for_each_chunk<E>(
    total: i32,
    execution: ProceduralExecution,
    mut worker: impl FnMut(i32, i32) -> Result<(), crate::PointwiseError<E>>,
) -> Result<(), crate::PointwiseError<E>> {
    for (start, count) in chunks(total, execution)? {
        worker(start, count)?;
    }
    Ok(())
}

/// Source q9.b profile, not a forced calling-thread or fixed-success scheduler.
pub(crate) fn for_each_row_chunk<E>(
    total: i32,
    execution: ProceduralExecution,
    mut worker: impl FnMut(i32, i32) -> Result<(), crate::PointwiseError<E>>,
) -> Result<(), crate::PointwiseError<E>> {
    for (start, count) in row_chunks(total, execution)? {
        worker(start, count)?;
    }
    Ok(())
}

/// Large dense passes can give workers nonoverlapping row bands. The
/// supplied kernel owns the intra-row evaluation order and sees a local span.
pub(crate) fn for_each_dense_band<T: Send>(
    output: &mut [T],
    width: usize,
    execution: ProceduralExecution,
    worker: impl Fn(usize, &mut [T]) -> Result<(), ImageError> + Sync,
) -> Result<(), ImageError> {
    let rows = output.len() / width;
    let workers = match execution {
        ProceduralExecution::WorkerPool { processor_count } if output.len() >= 512 * 512 => {
            processor_count.clamp(1, 8) as usize
        }
        _ => 1,
    };
    if workers == 1 || rows < 2 {
        return worker(0, output);
    }
    let rows_per_band = rows.div_ceil(workers);
    std::thread::scope(|scope| {
        let mut tasks = Vec::with_capacity(workers);
        for (index, pixels) in output.chunks_mut(rows_per_band * width).enumerate() {
            let worker = &worker;
            tasks.push(scope.spawn(move || worker(index * rows_per_band, pixels)));
        }
        for task in tasks {
            task.join().expect("dense render worker")?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod row_profile_tests {
    use super::*;
    #[test]
    fn minimum_one_rows_use_source_partition_without_changing_default_fifty() {
        let execution = ProceduralExecution::WorkerPool { processor_count: 2 };
        assert_eq!(chunks::<()>(8, execution).unwrap(), vec![(0, 8)]);
        assert_eq!(
            chunks_with_minimum::<()>(8, execution, 1).unwrap(),
            vec![(0, 2), (2, 2), (4, 1), (5, 1), (6, 1), (7, 1)]
        );
        assert!(
            chunks_with_minimum::<()>(0, execution, 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            chunks_with_minimum::<()>(0, ProceduralExecution::CallingThread, 1).unwrap(),
            vec![(0, 0)]
        );
    }
}
