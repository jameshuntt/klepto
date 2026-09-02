
###
### `# original concept`

### `vision 0.0.0`
    talk about the file in a header mentioning deps associated with features with scope of effect of each use

### `# new additions` in progress
#
#### `generating features`
    auto generating features by configuring a selection of deps which fall into each named feature flag, somethign like:

    async = ["tokio", tokio-select"]

#
#### `workspace features management`
    enabling use at workspace level configuring code_passenger to manage features, something like:

    async = ["tokio", "tokio-select"]
    async-EXCLUDE_CRATES = []

###
    you could decide to manually enter each effected crate, something like:

    async = ["tokio", "tokio-select"]
    async-INCLUDE_CRATES = []
#
#### `auto docs`
    generates documentation above each:
     - struct
     - function,
     - impl function
     - trait
     - enum
     - type
     - all of it


###
    mentioning use cases both direct and indirect, in an effort to build out a tree that can 
###
    styling preferences to word selection and level of detail


##
---
##
---
##

# `# rationally this is diverse`

###
    the great programmer melting pot
    
    use cases are endless, if we work together, nobosy will have  dog shit ai comments that drift in style across a codebase