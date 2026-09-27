use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::sync::Arc;

use derive_more::Debug;
use eyre::{Result, anyhow, bail};
use num::{BigUint, ToPrimitive};
use surfer_translation_types::{
    VariableDirection, VariableEncoding, VariableIndex, VariableType, VariableValue,
};
use tracing::warn;
use wellen::{
    FileFormat, Hierarchy, ScopeType, Signal, SignalEncoding, SignalRef, SignalSource, Time,
    TimeTable, TimeTableIdx, Timescale, TimescaleUnit, Var, VarRef, VarType,
};

use crate::time::{TimeScale, TimeUnit};
use crate::user_signals::UserSignals;
use crate::variable_direction::VariableDirectionExt;
use crate::variable_index::VariableIndexExt;
use crate::wave_container::{
    MetaData, QueryResult, ScopeId, ScopeRef, ScopeRefExt, VarId, VariableMeta, VariableRef,
    VariableRefExt,
};

static UNIQUE_ID_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug)]
pub struct WellenContainer {
    #[debug(skip)]
    pub(crate) hierarchy: std::sync::Arc<Hierarchy>,
    /// the url of a remote server, None if waveforms are loaded locally
    server: Option<String>,
    /// selected file index on the remote server, None for local waveforms
    remote_file_index: Option<usize>,
    scopes: Vec<String>,
    vars: Vec<String>,
    varrefs: Vec<VariableRef>,
    pub(crate) signals: HashMap<SignalRef, Arc<Signal>>,
    /// keeps track of signals that need to be loaded once the body of the waveform file has been loaded
    signals_to_be_loaded: HashSet<SignalRef>,
    pub(crate) time_table: Arc<TimeTable>,
    #[debug(skip)]
    source: Option<SignalSource>,
    unique_id: u64,
    body_loaded: bool,
    /// Signals created by the user, shown in a virtual top-level scope.
    pub(crate) user_signals: UserSignals,
}

/// Returned by `load_variables` if we want to load the variables on a background thread.
/// This struct is currently only used by wellen
pub struct LoadSignalsCmd {
    signals: Vec<SignalRef>,
    from_unique_id: u64,
    payload: LoadSignalPayload,
}

pub enum HeaderResult {
    /// Result of locally parsing the header of a waveform file with wellen from a file.
    LocalFile(Box<wellen::viewers::HeaderResult<std::io::BufReader<std::fs::File>>>),
    /// Result of locally parsing the header of a waveform file with wellen from bytes.
    LocalBytes(Box<wellen::viewers::HeaderResult<std::io::Cursor<Vec<u8>>>>),
    /// Result of querying a remote surfer server (which has used wellen).
    Remote(std::sync::Arc<Hierarchy>, FileFormat, String, usize),
}

pub enum BodyResult {
    /// Result of locally parsing the body of a waveform file with wellen.
    Local(wellen::viewers::BodyResult),
    /// Result of querying a remote surfer server (which has used wellen).
    Remote(Vec<wellen::Time>, String),
}

pub enum LoadSignalPayload {
    Local(SignalSource, std::sync::Arc<Hierarchy>),
    Remote(String, usize),
}

impl LoadSignalsCmd {
    #[must_use]
    pub fn destruct(self) -> (Vec<SignalRef>, u64, LoadSignalPayload) {
        (self.signals, self.from_unique_id, self.payload)
    }
}

pub struct LoadSignalsResult {
    source: Option<SignalSource>,
    server: Option<String>,
    signals: Vec<Signal>,
    from_unique_id: u64,
}

impl LoadSignalsResult {
    #[must_use]
    pub fn local(source: SignalSource, signals: Vec<Signal>, from_unique_id: u64) -> Self {
        Self {
            source: Some(source),
            server: None,
            signals,
            from_unique_id,
        }
    }

    #[must_use]
    pub fn remote(server: String, signals: Vec<Signal>, from_unique_id: u64) -> Self {
        Self {
            source: None,
            server: Some(server),
            signals,
            from_unique_id,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.signals.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }
}

#[must_use]
pub fn convert_format(format: FileFormat) -> crate::WaveFormat {
    match format {
        FileFormat::Vcd => crate::WaveFormat::Vcd,
        FileFormat::Fst => crate::WaveFormat::Fst,
        FileFormat::Ghw => crate::WaveFormat::Ghw,
        FileFormat::Unknown => unreachable!("should never get here"),
    }
}

impl WellenContainer {
    pub fn new(
        hierarchy: std::sync::Arc<Hierarchy>,
        server: Option<String>,
        remote_file_index: Option<usize>,
    ) -> Self {
        // generate a list of names for all variables and scopes since they will be requested by the parser
        let h = &hierarchy;
        let scopes = h
            .all_scopes()
            .map(|r| h[r].full_name(h))
            .collect::<Vec<_>>();
        let vars: Vec<String> = h
            .all_vars()
            .map(|r| {
                let r = &h[r];
                if let Some(i) = r.index()
                    && i.width() == 1
                {
                    format!("{}[{}]", r.full_name(h), i.lsb())
                } else {
                    r.full_name(h)
                }
            })
            .collect::<Vec<_>>();
        let varrefs = vars
            .iter()
            .enumerate()
            .filter_map(|(n, name)| {
                let r = VarRef::from_index(n).unwrap();
                let var = &h[r];
                if var.var_type().is_parameter() {
                    None
                } else {
                    Some(VariableRef::from_hierarchy_string_with_id(
                        name,
                        VarId::Wellen(r),
                    ))
                }
            })
            .collect::<Vec<_>>();

        let unique_id = UNIQUE_ID_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        Self {
            hierarchy,
            server,
            remote_file_index,
            scopes,
            vars,
            varrefs,
            signals: HashMap::new(),
            signals_to_be_loaded: HashSet::new(),
            time_table: Arc::new(vec![]),
            source: None,
            unique_id,
            body_loaded: false,
            user_signals: UserSignals::default(),
        }
    }

    #[must_use]
    pub fn body_loaded(&self) -> bool {
        self.body_loaded
    }

    pub fn add_body(&mut self, body: BodyResult) -> Result<Option<LoadSignalsCmd>> {
        if self.body_loaded {
            bail!("Did we just parse the body twice? That should not happen!");
        }
        match body {
            BodyResult::Local(body) => {
                if self.server.is_some() {
                    bail!(
                        "We are connected to a server, but also received the result of parsing a file locally. Something is going wrong here!"
                    );
                }
                self.time_table = Arc::new(body.time_table);
                self.source = Some(body.source);
            }
            BodyResult::Remote(time_table, server) => {
                if let Some(old) = &self.server {
                    if old != &server {
                        bail!("Inconsistent server URLs: {old} vs. {server}");
                    }
                } else {
                    bail!("Missing server URL!");
                }
                self.time_table = Arc::new(time_table);
            }
        }
        self.body_loaded = true;

        // we might have to load some signals that the user has already added while the
        // body of the waveform file was being parser
        Ok(self.load_signals(&[]))
    }

    #[must_use]
    pub fn metadata(&self) -> MetaData {
        let timescale = self
            .hierarchy
            .timescale()
            .unwrap_or(Timescale::new(1, TimescaleUnit::Unknown));
        let date = None;
        MetaData {
            date,
            version: Some(self.hierarchy.version().to_string()),
            timescale: TimeScale {
                unit: TimeUnit::from(timescale.unit),
                multiplier: Some(timescale.factor),
            },
        }
    }

    #[must_use]
    pub fn max_timestamp(&self) -> Option<BigUint> {
        self.time_table.last().map(|t| BigUint::from(*t))
    }

    #[must_use]
    pub fn min_timestamp(&self) -> Option<BigUint> {
        self.time_table.first().map(|t| BigUint::from(*t))
    }

    #[must_use]
    pub fn is_fully_loaded(&self) -> bool {
        (self.source.is_some() || self.server.is_some()) && self.signals_to_be_loaded.is_empty()
    }

    #[must_use]
    pub fn variable_names(&self) -> Vec<String> {
        let mut names = self.vars.clone();
        names.extend(
            self.user_signals
                .variables()
                .iter()
                .map(VariableRefExt::full_path_string),
        );
        names
    }

    fn lookup_scope(&self, scope: &ScopeRef) -> Option<wellen::ScopeRef> {
        match scope.id {
            ScopeId::Wellen(id) => Some(id),
            ScopeId::None => self.hierarchy.lookup_scope(scope.strs()),
        }
    }

    fn has_scope(&self, scope: &ScopeRef) -> bool {
        match scope.id {
            ScopeId::Wellen(_) => true,
            ScopeId::None => self.hierarchy.lookup_scope(scope.strs()).is_some(),
        }
    }

    #[must_use]
    pub fn get_scope_type(&self, scope: &ScopeRef) -> Option<ScopeType> {
        self.lookup_scope(scope)
            .map(|scope_ref| self.hierarchy[scope_ref].scope_type())
    }

    #[must_use]
    pub fn variables(&self) -> Vec<VariableRef> {
        let mut variables = self.varrefs.clone();
        variables.extend(self.user_signals.variables());
        variables
    }

    pub fn variables_in_scope(&self, scope_ref: &ScopeRef) -> Vec<VariableRef> {
        if UserSignals::is_user_scope(scope_ref) {
            return self.user_signals.variables();
        }
        let h = &self.hierarchy;
        // special case of an empty scope means that we want to variables that are part of the toplevel
        if scope_ref.has_empty_strs() {
            h.vars()
                .filter(|id| !h[*id].var_type().is_parameter())
                .map(|id| {
                    let v = &h[id];
                    let index = v
                        .index()
                        .and_then(|i| if i.width() == 1 { Some(i.lsb()) } else { None });
                    VariableRef::new_with_id_and_index(
                        scope_ref.clone(),
                        v.name(h).to_string(),
                        VarId::Wellen(id),
                        index,
                    )
                })
                .collect::<Vec<_>>()
        } else {
            let scope = if let Some(id) = self.lookup_scope(scope_ref) {
                &h[id]
            } else {
                warn!("Found no scope '{scope_ref}'. Defaulting to no variables");
                return vec![];
            };
            scope
                .vars(h)
                .filter(|id| !h[*id].var_type().is_parameter())
                .map(|id| {
                    let v = &h[id];
                    let index = v
                        .index()
                        .and_then(|i| if i.width() == 1 { Some(i.lsb()) } else { None });
                    VariableRef::new_with_id_and_index(
                        scope_ref.clone(),
                        v.name(h).to_string(),
                        VarId::Wellen(id),
                        index,
                    )
                })
                .collect::<Vec<_>>()
        }
    }

    pub fn parameters_in_scope(&self, scope_ref: &ScopeRef) -> Vec<VariableRef> {
        let h = &self.hierarchy;
        // special case of an empty scope means that we want to variables that are part of the toplevel
        if scope_ref.strs().is_empty() {
            h.vars()
                .filter(|id| h[*id].var_type().is_parameter())
                .map(|id| {
                    let v = &h[id];
                    let index = v
                        .index()
                        .and_then(|i| if i.width() == 1 { Some(i.lsb()) } else { None });
                    VariableRef::new_with_id_and_index(
                        scope_ref.clone(),
                        v.name(h).to_string(),
                        VarId::Wellen(id),
                        index,
                    )
                })
                .collect::<Vec<_>>()
        } else {
            let scope = if let Some(id) = self.lookup_scope(scope_ref) {
                &h[id]
            } else {
                warn!("Found no scope '{scope_ref}'. Defaulting to no variables");
                return vec![];
            };
            scope
                .vars(h)
                .filter(|id| h[*id].var_type().is_parameter())
                .map(|id| {
                    let v = &h[id];
                    let index = v
                        .index()
                        .and_then(|i| if i.width() == 1 { Some(i.lsb()) } else { None });
                    VariableRef::new_with_id_and_index(
                        scope_ref.clone(),
                        v.name(h).to_string(),
                        VarId::Wellen(id),
                        index,
                    )
                })
                .collect::<Vec<_>>()
        }
    }

    pub fn no_variables_in_scope(&self, scope_ref: &ScopeRef) -> bool {
        let h = &self.hierarchy;
        // special case of an empty scope means that we want to variables that are part of the toplevel
        if scope_ref.has_empty_strs() {
            h.vars().next().is_none()
        } else {
            let scope = if let Some(id) = self.lookup_scope(scope_ref) {
                &h[id]
            } else {
                warn!("Found no scope '{scope_ref}'. Defaulting to no variables");
                return true;
            };
            scope.vars(h).next().is_none()
        }
    }

    #[must_use]
    pub fn update_variable_ref(&self, variable: &VariableRef) -> Option<VariableRef> {
        if UserSignals::is_user_scope(&variable.path) {
            return self.user_signals.update_variable_ref(variable);
        }
        // IMPORTANT: lookup by name! Also consider index if a single-digit index is provided.
        let h = &self.hierarchy;
        let index = variable
            .index
            .as_ref()
            .map(|i| wellen::VarIndex::new(*i, *i));
        let (var, new_scope_ref) = if variable.path.has_empty_strs() {
            // lookup the variable with index if provided
            if let Some(var) = h.lookup_var_with_index(&[], &variable.name, &index) {
                (var, variable.path.clone())
            } else {
                // fallback to lookup without index
                let var = h.lookup_var(&[], &variable.name)?;
                (var, variable.path.clone())
            }
        } else {
            // first we lookup the scope in order to update the scope reference
            let scope = h.lookup_scope(variable.path.strs())?;
            let new_scope_ref = variable.path.with_id(ScopeId::Wellen(scope));

            // now we lookup the variable with index if provided
            let var = h[scope].vars(h).find(|r| {
                h[*r].name(h) == variable.name && {
                    let var_index = h[*r].index();
                    // match either exact index, or if no index is provided, match only variables with length >= 2
                    var_index == index
                        || (index.is_none() && var_index.is_some_and(|i| i.width() >= 2))
                }
            })?;
            (var, new_scope_ref)
        };

        let new_variable_ref = VariableRef::new_with_id_and_index(
            new_scope_ref,
            variable.name.clone(),
            VarId::Wellen(var),
            variable.index,
        );
        Some(new_variable_ref)
    }

    pub fn get_var(&self, r: &VariableRef) -> Result<&Var> {
        let h = &self.hierarchy;
        self.get_var_ref(r).map(|r| &h[r])
    }

    #[must_use]
    pub fn get_enum_map(&self, v: &Var) -> HashMap<String, String> {
        match v.enum_type(&self.hierarchy) {
            None => HashMap::new(),
            Some((_, mapping)) => HashMap::from_iter(
                mapping
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            ),
        }
    }

    pub(crate) fn get_var_ref(&self, r: &VariableRef) -> Result<VarRef> {
        match r.id {
            VarId::Wellen(id) => Ok(id),
            VarId::User(_) => bail!("{r:?} is a user signal, not a wellen variable"),
            VarId::None => {
                let h = &self.hierarchy;
                let index = r.index.as_ref().map(|i| wellen::VarIndex::new(*i, *i));

                let Some(var) = h.lookup_var_with_index(r.path.strs(), r.name.clone(), &index)
                else {
                    bail!("Failed to find variable: {r:?}");
                };
                Ok(var)
            }
        }
    }

    pub fn load_variables<S: AsRef<VariableRef>, T: Iterator<Item = S>>(
        &mut self,
        variables: T,
    ) -> Result<Option<LoadSignalsCmd>> {
        let h = &self.hierarchy;
        let signal_refs = variables
            .flat_map(|s| {
                let r = s.as_ref();
                self.get_var_ref(r).map(|v| h[v].signal_ref())
            })
            .collect::<Vec<_>>();
        Ok(self.load_signals(&signal_refs))
    }

    pub fn load_all_params(&mut self) -> Result<Option<LoadSignalsCmd>> {
        let h = &self.hierarchy;
        let params = h
            .all_vars()
            .map(|r| &h[r])
            .filter(|r| r.var_type().is_parameter())
            .map(wellen::Var::signal_ref)
            .collect::<Vec<_>>();
        Ok(self.load_signals(&params))
    }

    pub fn on_signals_loaded(&mut self, res: LoadSignalsResult) -> Result<Option<LoadSignalsCmd>> {
        // check to see if this command came from our container, or from a previous file that was open
        if res.from_unique_id == self.unique_id {
            // return source or server
            debug_assert!(self.source.is_none());
            debug_assert!(self.server.is_none());
            self.source = res.source;
            self.server = res.server;
            debug_assert!(self.server.is_some() || self.source.is_some());
            // install signals
            for signal in res.signals {
                self.signals.insert(signal.signal_ref(), Arc::new(signal));
            }
        }

        // see if there are any more signals to dispatch
        Ok(self.load_signals(&[]))
    }

    fn load_signals(&mut self, ids: &[SignalRef]) -> Option<LoadSignalsCmd> {
        // make sure that we do not load signals that have already been loaded
        let filtered_ids = ids
            .iter()
            .filter(|id| !self.signals.contains_key(id) && !self.signals_to_be_loaded.contains(id))
            .copied()
            .collect::<Vec<_>>();

        // add signals to signals that need to be loaded
        self.signals_to_be_loaded.extend(filtered_ids.iter());

        if self.signals_to_be_loaded.is_empty() {
            return None; // nothing to do here
        }

        if !self.body_loaded {
            return None; // it only makes sense to load signals after we have loaded the body
        }

        // we remove the server name in order to ensure that we do not load the same signal twice
        if let Some(server) = std::mem::take(&mut self.server) {
            let Some(file_index) = self.remote_file_index else {
                warn!("Missing remote file index while loading signals from {server}");
                return None;
            };
            // load remote signals
            let mut signals = self.signals_to_be_loaded.drain().collect::<Vec<_>>();
            signals.sort(); // for some determinism!
            let cmd = LoadSignalsCmd {
                signals,
                payload: LoadSignalPayload::Remote(server, file_index),
                from_unique_id: self.unique_id,
            };
            Some(cmd)
        } else if let Some(source) = std::mem::take(&mut self.source) {
            // if we have a source available, let's load all signals!
            let mut signals = self.signals_to_be_loaded.drain().collect::<Vec<_>>();
            signals.sort(); // for some determinism!
            let cmd = LoadSignalsCmd {
                signals,
                payload: LoadSignalPayload::Local(source, self.hierarchy.clone()),
                from_unique_id: self.unique_id,
            };
            Some(cmd)
        } else {
            None
        }
    }

    fn time_to_time_table_idx(&self, time: &BigUint) -> Option<TimeTableIdx> {
        let time: Time = time.to_u64().expect("unsupported time!");
        let table = &self.time_table;
        if table.is_empty() || table[0] > time {
            None
        } else {
            // binary search to find correct index
            let idx = binary_search(table, time);
            assert!(table[idx] <= time);
            Some(idx as TimeTableIdx)
        }
    }

    pub fn query_variable(
        &self,
        variable: &VariableRef,
        time: &BigUint,
    ) -> Result<Option<QueryResult>> {
        if let Some(idx) = self.user_signals.index_of(variable) {
            return Ok(self.user_signals.query(idx, time));
        }
        let h = &self.hierarchy;
        // find variable from string
        let var_ref = self.get_var_ref(variable)?;
        // map variable to variable ref
        let signal_ref = h[var_ref].signal_ref();
        let Some(sig) = self.signals.get(&signal_ref) else {
            // if the signal has not been loaded yet, we return an empty result
            return Ok(None);
        };
        let time_table = &self.time_table;

        // convert time to index
        if let Some(idx) = self.time_to_time_table_idx(time) {
            // get data offset
            if let Some(offset) = sig.get_offset(idx) {
                // which time did we actually get the value for?
                let offset_time_idx = sig.get_time_idx_at(&offset);
                let offset_time = time_table[offset_time_idx as usize];
                // get the last value in a time step (since we ignore delta cycles for now)
                let current_value = sig.get_value_at(&offset, offset.elements - 1);
                // the next time the variable changes
                let next_time = offset
                    .next_index
                    .and_then(|i| time_table.get(i.get() as usize));

                let converted_value = convert_variable_value(current_value);
                let result = QueryResult {
                    current: Some((BigUint::from(offset_time), converted_value)),
                    next: next_time.map(|t| BigUint::from(*t)),
                };
                return Ok(Some(result));
            }
        }

        // if `get_offset` returns None, this means that there is no change at or before the requested time
        let first_index = sig.get_first_time_idx();
        let next_time = first_index.and_then(|i| time_table.get(i as usize));
        let result = QueryResult {
            current: None,
            next: next_time.map(|t| BigUint::from(*t)),
        };
        Ok(Some(result))
    }

    #[must_use]
    pub fn scope_names(&self) -> Vec<String> {
        self.scopes.clone()
    }

    #[must_use]
    pub fn array_scope_names(&self) -> Vec<String> {
        let h = &self.hierarchy;

        fn collect_array_scopes(h: &Hierarchy, scope_id: wellen::ScopeRef, out: &mut Vec<String>) {
            let scope = &h[scope_id];
            if matches!(
                scope.scope_type(),
                ScopeType::VhdlArray | ScopeType::SvArray
            ) {
                out.push(scope.full_name(h));
            }

            for child in scope.scopes(h) {
                collect_array_scopes(h, child, out);
            }
        }

        let mut out = Vec::new();
        for root in h.scopes() {
            collect_array_scopes(h, root, &mut out);
        }
        out
    }

    #[must_use]
    pub fn root_scopes(&self) -> Vec<ScopeRef> {
        let h = &self.hierarchy;
        let mut scopes = h
            .scopes()
            .map(|id| ScopeRef::from_strs_with_id(&[h[id].name(h)], ScopeId::Wellen(id)))
            .collect::<Vec<_>>();
        if !self.user_signals.is_empty() {
            scopes.push(UserSignals::scope());
        }
        scopes
    }

    pub fn child_scopes(&self, scope_ref: &ScopeRef) -> Result<Vec<ScopeRef>> {
        if UserSignals::is_user_scope(scope_ref) {
            return Ok(vec![]);
        }
        let h = &self.hierarchy;
        let scope = match self.lookup_scope(scope_ref) {
            Some(id) => &h[id],
            None => return Err(anyhow!("Failed to find scope {scope_ref:?}")),
        };
        Ok(scope
            .scopes(h)
            .map(|id| scope_ref.with_subscope(h[id].name(h).to_string(), ScopeId::Wellen(id)))
            .collect::<Vec<_>>())
    }

    #[must_use]
    pub fn scope_exists(&self, scope: &ScopeRef) -> bool {
        if UserSignals::is_user_scope(scope) {
            return !self.user_signals.is_empty();
        }
        scope.has_empty_strs() || self.has_scope(scope)
    }

    #[must_use]
    /// True if the scope represents a compound variable
    pub fn scope_is_variable(&self, scope: &ScopeRef) -> bool {
        if let Some(scope_ref) = self.lookup_scope(scope) {
            let h = &self.hierarchy;
            let scope = &h[scope_ref];
            matches!(
                scope.scope_type(),
                ScopeType::Struct
                    | ScopeType::Union
                    | ScopeType::Class
                    | ScopeType::Interface
                    | ScopeType::VhdlRecord
                    | ScopeType::VhdlArray
                    | ScopeType::SvArray
            )
        } else {
            false
        }
    }

    #[must_use]
    /// True if the scope represents an array
    pub fn scope_is_array(&self, scope: &ScopeRef) -> bool {
        if let Some(scope_ref) = self.lookup_scope(scope) {
            let h = &self.hierarchy;
            let scope = &h[scope_ref];
            matches!(
                scope.scope_type(),
                ScopeType::VhdlArray | ScopeType::SvArray
            )
        } else {
            false
        }
    }

    #[must_use]
    pub fn get_scope_tooltip_data(&self, scope: &ScopeRef) -> String {
        let mut out = String::new();
        if let Some(scope_ref) = self.lookup_scope(scope) {
            let h = &self.hierarchy;
            let scope = &h[scope_ref];
            writeln!(&mut out, "{}", scope_type_to_string(scope.scope_type())).unwrap();
            if let Some((path, line)) = scope.instantiation_source_loc(h) {
                writeln!(&mut out, "{path}:{line}").unwrap();
            }
            match (scope.component(h), scope.source_loc(h)) {
                (Some(name), Some((path, line))) => {
                    write!(&mut out, "{name} : {path}:{line}").unwrap();
                }
                (None, Some((path, line))) => {
                    // check to see if instance and definition are the same
                    let same = scope
                        .instantiation_source_loc(h)
                        .is_some_and(|(i_path, i_line)| path == i_path && line == i_line);
                    if !same {
                        write!(&mut out, "{path}:{line}").unwrap();
                    }
                }
                (Some(name), None) => write!(&mut out, "{name}").unwrap(),
                // remove possible trailing new line
                (None, None) => {}
            }
        }
        if out.ends_with('\n') {
            out.pop().unwrap();
        }
        out
    }

    pub fn variable_to_meta(&self, variable: &VariableRef) -> Result<VariableMeta> {
        if self.user_signals.index_of(variable).is_some() {
            return self.user_signals.meta(variable);
        }
        let var = self.get_var(variable)?;
        let encoding = match var.signal_encoding(&self.hierarchy) {
            SignalEncoding::String => VariableEncoding::String,
            SignalEncoding::Real => VariableEncoding::Real,
            SignalEncoding::BitVector(_) => VariableEncoding::BitVector,
        };
        Ok(VariableMeta {
            var: variable.clone(),
            num_bits: var.length(&self.hierarchy),
            variable_type: Some(VariableType::from_wellen_type(var.var_type())),
            variable_type_name: var.vhdl_type_name(&self.hierarchy).map(ToString::to_string),
            index: var.index().map(VariableIndex::from_wellen_type),
            direction: Some(VariableDirection::from_wellen_direction(var.direction())),
            enum_map: self.get_enum_map(var),
            encoding,
        })
    }

    pub fn signal_accessor(&self, signal_ref: SignalRef) -> Result<WellenSignalAccessor> {
        let signal = self
            .signals
            .get(&signal_ref)
            .cloned()
            .ok_or_else(|| anyhow!("Signal not loaded"))?;
        Ok(WellenSignalAccessor::new(
            signal,
            Arc::clone(&self.time_table),
        ))
    }

    /// Get the `SignalRef` for a variable (canonical signal identity for cache keys)
    pub fn signal_ref(&self, variable: &VariableRef) -> Result<SignalRef> {
        let var_ref = self.get_var_ref(variable)?;
        Ok(self.hierarchy[var_ref].signal_ref())
    }

    /// Check if a signal is already loaded (data available)
    #[must_use]
    pub fn is_signal_loaded(&self, signal_ref: SignalRef) -> bool {
        self.signals.contains_key(&signal_ref)
    }
}

/// Wellen-specific accessor for iterating through signal changes in a time range
pub struct WellenSignalAccessor {
    signal: Arc<Signal>,
    time_table: Arc<TimeTable>,
}

impl WellenSignalAccessor {
    /// Create a new `WellenSignalAccessor` from Arc pointers
    #[must_use]
    pub fn new(signal: Arc<Signal>, time_table: Arc<TimeTable>) -> Self {
        Self { signal, time_table }
    }

    /// Iterator over signal changes as (`time_u64`, value) pairs
    pub fn iter_changes(
        &self,
    ) -> Box<dyn Iterator<Item = (u64, surfer_translation_types::VariableValue)> + '_> {
        Box::new(
            self.signal
                .iter_changes()
                .filter_map(|(time_idx, signal_value)| {
                    let time_u64 = *self.time_table.get(time_idx as usize)?;
                    let var_value = convert_variable_value(signal_value);
                    Some((time_u64, var_value))
                }),
        )
    }
}

fn scope_type_to_string(tpe: ScopeType) -> &'static str {
    match tpe {
        ScopeType::Module => "module",
        ScopeType::Task => "task",
        ScopeType::Function | ScopeType::VhdlFunction => "function",
        ScopeType::Begin => "begin",
        ScopeType::Fork => "fork",
        ScopeType::Generate | ScopeType::VhdlGenerate => "generate",
        ScopeType::Struct => "struct",
        ScopeType::Union => "union",
        ScopeType::Class => "class",
        ScopeType::Interface => "interface",
        ScopeType::Package | ScopeType::VhdlPackage => "package",
        ScopeType::Program => "program",
        ScopeType::VhdlArchitecture => "architecture",
        ScopeType::VhdlProcedure => "procedure",
        ScopeType::VhdlRecord => "record",
        ScopeType::VhdlProcess => "process",
        ScopeType::VhdlBlock => "block",
        ScopeType::VhdlForGenerate => "for-generate",
        ScopeType::VhdlIfGenerate => "if-generate",
        ScopeType::GhwGeneric => "generic",
        ScopeType::VhdlArray | ScopeType::SvArray => "array",
        ScopeType::Unknown => "unknown",
        _ => todo!(),
    }
}

fn convert_variable_value(value: wellen::SignalValueRef) -> VariableValue {
    match value {
        wellen::SignalValueRef::BitVec(bv) => {
            if let Some(be_bytes) = bv.be_bytes() {
                VariableValue::BigUint(BigUint::from_bytes_be(be_bytes))
            } else {
                VariableValue::String(bv.bit_string())
            }
        }
        wellen::SignalValueRef::String(value) => VariableValue::String(value.to_string()),
        wellen::SignalValueRef::Real(value) => {
            VariableValue::BigUint(BigUint::from(value.to_bits()))
        }
        wellen::SignalValueRef::Event => VariableValue::String("Event".to_string()),
    }
}

#[local_impl::local_impl]
impl FromVarType for VariableType {
    fn from_wellen_type(signaltype: VarType) -> Self {
        match signaltype {
            VarType::Reg => VariableType::VCDReg,
            VarType::Wire => VariableType::VCDWire,
            VarType::Integer => VariableType::VCDInteger,
            VarType::Real => VariableType::VCDReal,
            VarType::Parameter => VariableType::VCDParameter,
            VarType::String => VariableType::VCDString,
            VarType::Time => VariableType::VCDTime,
            VarType::Event => VariableType::VCDEvent,
            VarType::Supply0 => VariableType::VCDSupply0,
            VarType::Supply1 => VariableType::VCDSupply1,
            VarType::Tri => VariableType::VCDTri,
            VarType::TriAnd => VariableType::VCDTriAnd,
            VarType::TriOr => VariableType::VCDTriOr,
            VarType::TriReg => VariableType::VCDTriReg,
            VarType::Tri0 => VariableType::VCDTri0,
            VarType::Tri1 => VariableType::VCDTri1,
            VarType::WAnd => VariableType::VCDWAnd,
            VarType::WOr => VariableType::VCDWOr,
            VarType::Port => VariableType::Port,
            VarType::Bit => VariableType::Bit,
            VarType::Logic => VariableType::Logic,
            VarType::Int => VariableType::Int,
            VarType::Enum => VariableType::Enum,
            VarType::SparseArray => VariableType::SparseArray,
            VarType::RealTime => VariableType::RealTime,
            VarType::ShortInt => VariableType::ShortInt,
            VarType::LongInt => VariableType::LongInt,
            VarType::Byte => VariableType::Byte,
            VarType::ShortReal => VariableType::ShortReal,
            VarType::Boolean => VariableType::Boolean,
            VarType::BitVector => VariableType::BitVector,
            VarType::StdLogic => VariableType::StdLogic,
            VarType::StdLogicVector => VariableType::StdLogicVector,
            VarType::StdULogic => VariableType::StdULogic,
            VarType::StdULogicVector => VariableType::StdULogicVector,
            VarType::RealParameter => VariableType::RealParameter,
            VarType::EventParameter => VariableType::EventParameter,
        }
    }
}

#[local_impl::local_impl]
impl ToVarType for VariableType {
    fn to_wellen_type(&self) -> VarType {
        match self {
            VariableType::VCDReg => VarType::Reg,
            VariableType::VCDWire => VarType::Wire,
            VariableType::VCDInteger => VarType::Integer,
            VariableType::VCDReal => VarType::Real,
            VariableType::VCDParameter => VarType::Parameter,
            VariableType::VCDString => VarType::String,
            VariableType::VCDTime => VarType::Time,
            VariableType::VCDEvent => VarType::Event,
            VariableType::VCDSupply0 => VarType::Supply0,
            VariableType::VCDSupply1 => VarType::Supply1,
            VariableType::VCDTri => VarType::Tri,
            VariableType::VCDTriAnd => VarType::TriAnd,
            VariableType::VCDTriOr => VarType::TriOr,
            VariableType::VCDTriReg => VarType::TriReg,
            VariableType::VCDTri0 => VarType::Tri0,
            VariableType::VCDTri1 => VarType::Tri1,
            VariableType::VCDWAnd => VarType::WAnd,
            VariableType::VCDWOr => VarType::WOr,
            VariableType::Port => VarType::Port,
            VariableType::Bit => VarType::Bit,
            VariableType::Logic => VarType::Logic,
            VariableType::Int => VarType::Int,
            VariableType::Enum => VarType::Enum,
            VariableType::SparseArray => VarType::SparseArray,
            VariableType::ShortInt => VarType::ShortInt,
            VariableType::LongInt => VarType::LongInt,
            VariableType::Byte => VarType::Byte,
            VariableType::ShortReal => VarType::ShortReal,
            VariableType::Boolean => VarType::Boolean,
            VariableType::BitVector => VarType::BitVector,
            VariableType::StdLogic => VarType::StdLogic,
            VariableType::StdLogicVector => VarType::StdLogicVector,
            VariableType::StdULogic => VarType::StdULogic,
            VariableType::StdULogicVector => VarType::StdULogicVector,
            VariableType::RealParameter => VarType::RealParameter,
            VariableType::EventParameter => VarType::EventParameter,
            VariableType::RealTime => VarType::RealTime,
        }
    }
}

#[local_impl::local_impl]
impl VarTypeExt for VarType {
    fn is_parameter(&self) -> bool {
        matches!(self, VarType::Parameter | VarType::RealParameter)
    }
}

#[inline]
fn binary_search(times: &[Time], needle: Time) -> usize {
    let mut lower_idx = 0usize;
    let mut upper_idx = times.len() - 1;
    while lower_idx <= upper_idx {
        let mid_idx = lower_idx + ((upper_idx - lower_idx) / 2);

        match times[mid_idx].cmp(&needle) {
            std::cmp::Ordering::Less => {
                lower_idx = mid_idx + 1;
            }
            std::cmp::Ordering::Equal => {
                return mid_idx;
            }
            std::cmp::Ordering::Greater => {
                upper_idx = mid_idx - 1;
            }
        }
    }
    lower_idx - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellen::States;

    #[test]
    fn test_signal_conversion() {
        let inp0: &[u8] = &[128, 0, 0, 3];
        let out0 = convert_variable_value(wellen::SignalValueRef::bit_vec(States::Two, 32, inp0));
        assert_eq!(out0, VariableValue::BigUint(BigUint::from(0x80000003u64)));
    }
}
