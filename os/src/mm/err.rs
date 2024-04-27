use core::fmt::Display;

use alloc::string::ToString;

/// Errors related to page permission
#[derive(Debug)]
pub enum PagePermissionError {
    /// not executable
    NotExecutable,
    /// not readable
    NotReadable,
    /// not writable
    NotWritable,
    /// not user accessible
    NotUserAccessible,
}

/// Errors related to page mapping
#[derive(Debug)]
pub enum PageError {
    /// returned when intermediate directory pages are invalid,
    /// often returned from some non-creating search process.
    InvalidDirPage,

    /// indicates that the requested virtual page has already been `valid`
    PageAlreadyValid,

    /// indicates that the requested virtual page has not yet been `valid`
    PageInvalid,

    /// indicates that operation is to a page of unexpected permission
    PageUnexpectedPermission(PagePermissionError)
}

/// Errors related to area management
#[derive(Debug)]
pub enum AreaError {
    /// no requested area
    NoMatchingArea,
    /// requested area contains mapped portion,
    /// often returned from some mapping procsess.
    AreaHasMappedPortion,
    /// requested area contains unmapped portion,
    /// often returned from some unmapping process.
    AreaHasUnmappedPortion,
    /// when trying to unmap a critical area, e.g. `TRAMPOLINE`
    AreaCritical,
    /// when requested vpn is not inside the area
    AreaRangeNotInclude,
}

/// Errors related to memory management
#[derive(Debug)]
pub enum MMError {
    /// when allocatable memory too low
    NotEnoughMemory,
    /// page error
    PageError(PageError),
    /// area error
    AreaError(AreaError)
}

/// Wrapped `Result` for `MMError`
pub type MMResult<R> = core::result::Result<R, MMError>;

impl Display for MMError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MMError::NotEnoughMemory => f.write_str("NotEnoughMemory"),
            MMError::PageError(pe) => f.write_str(pe.to_string().as_str()),
            MMError::AreaError(ae) => f.write_str(ae.to_string().as_str()),
        }
    }
}

impl Display for PagePermissionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PagePermissionError::NotExecutable => f.write_str("NotExecutable"),
            PagePermissionError::NotReadable => f.write_str("NotReadable"),
            PagePermissionError::NotWritable => f.write_str("NotWritable"),
            PagePermissionError::NotUserAccessible => f.write_str("NotUserAccessible"),
        }
    }
}

impl Display for PageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PageError::InvalidDirPage => f.write_str("InvalidDirPage"),
            PageError::PageAlreadyValid => f.write_str("PageAlreadyValid"),
            PageError::PageInvalid => f.write_str("PageInvalid"),
            PageError::PageUnexpectedPermission(e) => f.write_str(e.to_string().as_str()),
        }
    }
}

impl Display for AreaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AreaError::NoMatchingArea => f.write_str("NoMatchingArea"),
            AreaError::AreaHasMappedPortion => f.write_str("AreaHasMappedPortion"),
            AreaError::AreaHasUnmappedPortion => f.write_str("AreaHasUnmappedPortion"),
            AreaError::AreaCritical => f.write_str("AreaCritical"),
            AreaError::AreaRangeNotInclude => f.write_str("AreaRangeNotInclude"),
        }
    }
}

impl From<PageError> for MMError {
    fn from(value: PageError) -> Self {
        Self::PageError(value)
    }
}
impl From<AreaError> for MMError {
    fn from(value: AreaError) -> Self {
        Self::AreaError(value)
    }
}
impl From<PagePermissionError> for MMError {
    fn from(value: PagePermissionError) -> Self {
        Self::PageError(PageError::PageUnexpectedPermission(value))
    }
}