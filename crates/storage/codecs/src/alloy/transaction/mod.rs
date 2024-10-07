pub(crate) mod eip1559;
pub(crate) mod eip2930;
pub(crate) mod eip4844;
pub(crate) mod eip7702;
pub(crate) mod legacy;
#[cfg(feature = "optimism")]
pub(crate) mod optimism;

#[cfg(test)]
mod tests {

    // each value in the database has an extra field named flags that encodes metadata about other
    // fields in the value, e.g. offset and length.
    //
    // this check is to ensure we do not inadvertently add too many fields to a struct which would
    // expand the flags field and break backwards compatibility
}
