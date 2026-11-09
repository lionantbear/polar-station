mod codec;
mod model;
pub use codec::{decode_cas, encode_idea, parse_idea, EncodedIdea};
pub use model::{IdeaTree, Limits, Node, Relation};

pub(crate) use codec::strict_value;
pub(crate) use model::check_values;

mod edit;
pub use edit::{replace_reference, replace_local_occurrence};
