//! The concrete za capacity policy over caller-supplied native observations.
//! No operating-system provider or success-shaped memory default lives here.
//! Source: Beta za.cs415–444,1107–1116,1235–1280,1298–1322; qualified CIL
//! facts are preserved in port/.scratch/mycela-memory-source. This is not a
//! LowMemoryEmergency/preflight/retry/spilling/GC implementation.
use std::fmt;

/// Explicit source address-cap profile. The original PE initializes Windows;
/// a caller using another native OS must name its policy extension separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryAddressSpace {
    Windows { pointer_bytes: u8 },
    MacOs,
    Other,
}

/// Actual lazy native observations. Only the available-physical query is caught
/// by the policy; its reporter runs BEFORE fallback and may itself fail.
pub trait MemoryCapacityEffects {
    type Error;
    fn working_set_bytes(&mut self) -> Result<i64, Self::Error>;
    fn available_physical_bytes(&mut self) -> Result<u64, Self::Error>;
    /// Local DateTime ticks (100ns since 0001-01-01), not monotonic elapsed time.
    fn now_local_ticks(&mut self) -> Result<i64, Self::Error>;
    fn report_available_physical_error(&mut self, error: Self::Error) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryCapacityError<E> {
    Query(E),
    UnsupportedAddressSpace,
}
impl<E: fmt::Display> fmt::Display for MemoryCapacityError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query(error) => error.fmt(f),
            Self::UnsupportedAddressSpace => f.write_str(
                "The source memory policy has no address-cap profile for this operating system.",
            ),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for MemoryCapacityError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query(error) => Some(error),
            Self::UnsupportedAddressSpace => None,
        }
    }
}

/// One owner retains the process-policy cache across consumers. This type has
/// no internal lock and claims no parity for the source's unsynchronized races.
/// Callers must release any policy guard before executing a render kernel or
/// invoking unrelated application callbacks.
#[derive(Debug)]
pub struct MemoryCapacityPolicy {
    profile: MemoryAddressSpace,
    address_limit_mib: i32,
    previous_available_mib: i32,
    cached_budget_mib: i32,
    last_refresh_local_ticks: i64,
    pressure_factor: f64,
}
impl MemoryCapacityPolicy {
    pub fn new(profile: MemoryAddressSpace) -> Self {
        Self {
            profile,
            address_limit_mib: -1,
            previous_available_mib: 800,
            cached_budget_mib: -1,
            last_refresh_local_ticks: 0,
            pressure_factor: 1.0,
        }
    }
    pub fn cached_address_limit_mib(&self) -> i32 {
        self.address_limit_mib
    }
    pub fn previous_available_mib(&self) -> i32 {
        self.previous_available_mib
    }
    pub fn cached_budget_mib(&self) -> i32 {
        self.cached_budget_mib
    }
    pub fn last_refresh_local_ticks(&self) -> i64 {
        self.last_refresh_local_ticks
    }
    pub fn pressure_factor(&self) -> f64 {
        self.pressure_factor
    }
    /// Actual public za.g primitive only. Emergency callbacks, retry allocation,
    /// sleeping, spilling and GC must not be inferred from this increment.
    pub fn increase_pressure(&mut self) {
        self.pressure_factor += 0.2;
    }

    /// Literal za.a(width,oi) predicate. Quality is the underlying oi ordinal:
    /// 0 halves28, 1 divides by1.4, 3 multiplies by1.4, 4 doubles; 2 and every
    /// unknown ordinal keep28. Combine supplies ordinal4. No height enters it.
    pub fn capacity_available<Q: MemoryCapacityEffects>(
        &mut self,
        width: i32,
        quality_ordinal: i32,
        effects: &mut Q,
    ) -> Result<bool, MemoryCapacityError<Q::Error>> {
        if width < 500 {
            return Ok(true);
        }
        // This first WorkingSet is retained for the final comparison. The
        // refresh's second WorkingSet must never replace or reuse this sample.
        let working = signed_mib(
            effects
                .working_set_bytes()
                .map_err(MemoryCapacityError::Query)?,
        );
        let estimate = reserve_estimate(width, quality_ordinal, self.pressure_factor);
        let budget = self.budget(effects)?;
        let floor = crate::framework_cast::integer(f64::from(budget) * 0.3);
        let reserve = if estimate < floor { floor } else { estimate };
        let remaining = budget.wrapping_sub(reserve);
        Ok(working <= if remaining < 0 { 0 } else { remaining })
    }
    fn address_limit<E>(&mut self) -> Result<i32, MemoryCapacityError<E>> {
        if self.address_limit_mib < 0 {
            self.address_limit_mib = match self.profile {
                MemoryAddressSpace::Windows { pointer_bytes } => {
                    if pointer_bytes > 4 {
                        8_388_608
                    } else {
                        2048
                    }
                }
                MemoryAddressSpace::MacOs => 4096,
                MemoryAddressSpace::Other => {
                    return Err(MemoryCapacityError::UnsupportedAddressSpace);
                }
            };
        }
        Ok(self.address_limit_mib)
    }
    fn available<Q: MemoryCapacityEffects>(
        &mut self,
        effects: &mut Q,
    ) -> Result<i32, MemoryCapacityError<Q::Error>> {
        let mut available = match effects.available_physical_bytes() {
            Ok(bytes) => unsigned_mib(bytes),
            Err(error) => {
                effects
                    .report_available_physical_error(error)
                    .map_err(MemoryCapacityError::Query)?;
                self.previous_available_mib
            }
        };
        // The source catch ends before the cap and store; an unsupported cap
        // must not turn a successful native observation into committed history.
        if available > self.address_limit()? {
            available = self.address_limit()?;
        }
        self.previous_available_mib = available;
        Ok(available)
    }
    fn budget<Q: MemoryCapacityEffects>(
        &mut self,
        effects: &mut Q,
    ) -> Result<i32, MemoryCapacityError<Q::Error>> {
        // DateTime.Now is read even when the cache is still negative.
        let now = effects
            .now_local_ticks()
            .map_err(MemoryCapacityError::Query)?;
        let elapsed_seconds = (now.wrapping_sub(self.last_refresh_local_ticks) as f64) * 0.0000001;
        if self.cached_budget_mib < 0 || elapsed_seconds > 30.0 {
            let available = self.available(effects)?;
            let working = signed_mib(
                effects
                    .working_set_bytes()
                    .map_err(MemoryCapacityError::Query)?,
            );
            let combined = available.wrapping_add(working);
            let limit = self.address_limit()?;
            self.cached_budget_mib = combined.min(limit);
            // A failure here leaves the NEW budget but OLD timestamp intact.
            self.last_refresh_local_ticks = effects
                .now_local_ticks()
                .map_err(MemoryCapacityError::Query)?;
        }
        Ok(self.cached_budget_mib)
    }
}
fn signed_mib(bytes: i64) -> i32 {
    crate::framework_cast::integer((bytes as f64) / 1_048_576.0)
}
fn unsigned_mib(bytes: u64) -> i32 {
    crate::framework_cast::integer((bytes as f64) / 1_048_576.0)
}
fn reserve_estimate(mut width: i32, quality_ordinal: i32, pressure: f64) -> i32 {
    if width > 100_000 {
        width = 100_000;
    }
    let mut coefficient = 28.0;
    match quality_ordinal {
        4 => coefficient *= 2.0,
        3 => coefficient *= 1.4,
        1 => coefficient /= 1.4,
        0 => coefficient /= 2.0,
        _ => {}
    }
    coefficient *= pressure;
    let square = width.wrapping_mul(width);
    crate::framework_cast::integer(f64::from(square) * coefficient / 1_000_000.0)
}

#[cfg(test)]
#[path = "memory_capacity_test.rs"]
mod tests;
