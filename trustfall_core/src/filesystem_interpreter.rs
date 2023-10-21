#![allow(dead_code)]

use std::fs::{self, ReadDir};
use std::iter;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::interpreter::AsVertex;
use crate::{
    interpreter::{
        Adapter, ContextIterator, ContextOutcomeIterator, DataContext, ResolveEdgeInfo,
        ResolveInfo, VertexIterator,
    },
    ir::{EdgeParameters, FieldValue},
};

#[derive(Debug, Clone)]
pub struct FilesystemInterpreter {
    origin: Rc<String>,
}

impl FilesystemInterpreter {
    pub fn new(origin: String) -> FilesystemInterpreter {
        FilesystemInterpreter { origin: Rc::new(origin) }
    }
}

#[derive(Debug)]
struct OriginIterator {
    origin_vertex: DirectoryVertex,
    produced: bool,
}

impl OriginIterator {
    pub fn new(vertex: DirectoryVertex) -> OriginIterator {
        OriginIterator { origin_vertex: vertex, produced: false }
    }
}

impl Iterator for OriginIterator {
    type Item = FilesystemVertex;

    fn next(&mut self) -> Option<FilesystemVertex> {
        if self.produced {
            None
        } else {
            self.produced = true;
            Some(FilesystemVertex::Directory(self.origin_vertex.clone()))
        }
    }
}

#[derive(Debug)]
struct DirectoryContainsFileIterator {
    origin: Rc<String>,
    directory: DirectoryVertex,
    file_iter: ReadDir,
}

impl DirectoryContainsFileIterator {
    pub fn new(origin: Rc<String>, directory: &DirectoryVertex) -> DirectoryContainsFileIterator {
        let mut buf = PathBuf::new();
        buf.extend([&*origin, &directory.path]);
        DirectoryContainsFileIterator {
            origin,
            directory: directory.clone(),
            file_iter: fs::read_dir(buf).unwrap(),
        }
    }
}

impl Iterator for DirectoryContainsFileIterator {
    type Item = FilesystemVertex;

    fn next(&mut self) -> Option<FilesystemVertex> {
        loop {
            if let Some(outcome) = self.file_iter.next() {
                match outcome {
                    Ok(dir_entry) => {
                        let metadata = match dir_entry.metadata() {
                            Ok(res) => res,
                            _ => continue,
                        };
                        if metadata.is_file() {
                            let name = dir_entry.file_name().to_str().unwrap().to_owned();
                            let mut buf = PathBuf::new();
                            buf.extend([&self.directory.path, &name]);
                            let extension = Path::new(&name)
                                .extension()
                                .map(|x| x.to_str().unwrap().to_owned());
                            let result = FileVertex {
                                name,
                                extension,
                                path: buf.to_str().unwrap().to_owned(),
                            };
                            return Some(FilesystemVertex::File(result));
                        }
                    }
                    _ => continue,
                }
            } else {
                return None;
            }
        }
    }
}

#[derive(Debug)]
struct SubdirectoryIterator {
    origin: Rc<String>,
    directory: DirectoryVertex,
    dir_iter: ReadDir,
}

impl SubdirectoryIterator {
    pub fn new(origin: Rc<String>, directory: &DirectoryVertex) -> Self {
        let mut buf = PathBuf::new();
        buf.extend([&*origin, &directory.path]);
        Self { origin, directory: directory.clone(), dir_iter: fs::read_dir(buf).unwrap() }
    }
}

impl Iterator for SubdirectoryIterator {
    type Item = FilesystemVertex;

    fn next(&mut self) -> Option<FilesystemVertex> {
        loop {
            if let Some(outcome) = self.dir_iter.next() {
                match outcome {
                    Ok(dir_entry) => {
                        let metadata = match dir_entry.metadata() {
                            Ok(res) => res,
                            _ => continue,
                        };
                        if metadata.is_dir() {
                            let name = dir_entry.file_name().to_str().unwrap().to_owned();
                            if name == ".git" || name == ".vscode" || name == "target" {
                                continue;
                            }

                            let mut buf = PathBuf::new();
                            buf.extend([&self.directory.path, &name]);
                            let result =
                                DirectoryVertex { name, path: buf.to_str().unwrap().to_owned() };
                            return Some(FilesystemVertex::Directory(result));
                        }
                    }
                    _ => continue,
                }
            } else {
                return None;
            }
        }
    }
}

pub type ContextAndValue = (DataContext<FilesystemVertex>, FieldValue);

type IndividualEdgeResolver<'a> =
    fn(Rc<String>, &FilesystemVertex) -> VertexIterator<'a, FilesystemVertex>;
type ContextAndIterableOfEdges<'a, V> = (DataContext<V>, VertexIterator<'a, FilesystemVertex>);

struct EdgeResolverIterator<'a, V: AsVertex<FilesystemVertex>> {
    origin: Rc<String>,
    contexts: VertexIterator<'a, DataContext<V>>,
    edge_resolver: IndividualEdgeResolver<'a>,
}

impl<'a, V: AsVertex<FilesystemVertex>> EdgeResolverIterator<'a, V> {
    pub fn new(
        origin: Rc<String>,
        contexts: VertexIterator<'a, DataContext<V>>,
        edge_resolver: IndividualEdgeResolver<'a>,
    ) -> Self {
        Self { origin, contexts, edge_resolver }
    }
}

impl<'a, V: AsVertex<FilesystemVertex>> Iterator for EdgeResolverIterator<'a, V> {
    type Item = (DataContext<V>, VertexIterator<'a, FilesystemVertex>);

    fn next(&mut self) -> Option<ContextAndIterableOfEdges<'a, V>> {
        if let Some(context) = self.contexts.next() {
            if let Some(vertex) = context.active_vertex::<FilesystemVertex>() {
                let neighbors = (self.edge_resolver)(self.origin.clone(), vertex);
                Some((context, neighbors))
            } else {
                let empty_iterator: iter::Empty<FilesystemVertex> = iter::empty();
                Some((context, Box::new(empty_iterator)))
            }
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesystemVertex {
    Directory(DirectoryVertex),
    File(FileVertex),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryVertex {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVertex {
    pub name: String,
    pub extension: Option<String>,
    pub path: String,
}

fn directory_contains_file_handler<'a>(
    origin: Rc<String>,
    vertex: &FilesystemVertex,
) -> VertexIterator<'a, FilesystemVertex> {
    let directory_vertex = match vertex {
        FilesystemVertex::Directory(dir) => dir,
        _ => unreachable!(),
    };
    Box::from(DirectoryContainsFileIterator::new(origin, directory_vertex))
}

fn directory_subdirectory_handler<'a>(
    origin: Rc<String>,
    vertex: &FilesystemVertex,
) -> VertexIterator<'a, FilesystemVertex> {
    let directory_vertex = match vertex {
        FilesystemVertex::Directory(dir) => dir,
        _ => unreachable!(),
    };
    Box::from(SubdirectoryIterator::new(origin, directory_vertex))
}

#[allow(unused_variables)]
impl<'a> Adapter<'a> for FilesystemInterpreter {
    type Vertex = FilesystemVertex;

    fn resolve_starting_vertices(
        &self,
        edge_name: &Arc<str>,
        parameters: &EdgeParameters,
        resolve_info: &ResolveInfo,
    ) -> VertexIterator<'a, Self::Vertex> {
        assert!(edge_name.as_ref() == "OriginDirectory");
        assert!(parameters.is_empty());
        let vertex = DirectoryVertex { name: "<origin>".to_owned(), path: "".to_owned() };
        Box::new(OriginIterator::new(vertex))
    }

    fn resolve_property<V: AsVertex<Self::Vertex> + 'a>(
        &self,
        contexts: ContextIterator<'a, V>,
        type_name: &Arc<str>,
        property_name: &Arc<str>,
        resolve_info: &ResolveInfo,
    ) -> ContextOutcomeIterator<'a, V, FieldValue> {
        match type_name.as_ref() {
            "Directory" => {
                match property_name.as_ref() {
                    "name" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(FilesystemVertex::Directory(ref x)) => {
                                let value = FieldValue::String(x.name.clone().into());
                                (context, value)
                            }
                            _ => unreachable!(),
                        }
                    })),
                    "path" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(FilesystemVertex::Directory(ref x)) => {
                                let value = FieldValue::String(x.path.clone().into());
                                (context, value)
                            }
                            _ => unreachable!(),
                        }
                    })),
                    "__typename" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(_) => (context, "Directory".into()),
                        }
                    })),
                    _ => todo!(),
                }
            }
            "File" => {
                match property_name.as_ref() {
                    "name" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(FilesystemVertex::File(ref x)) => {
                                let value = FieldValue::String(x.name.clone().into());
                                (context, value)
                            }
                            _ => unreachable!(),
                        }
                    })),
                    "path" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(FilesystemVertex::File(ref x)) => {
                                let value = FieldValue::String(x.path.clone().into());
                                (context, value)
                            }
                            _ => unreachable!(),
                        }
                    })),
                    "extension" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(FilesystemVertex::File(ref x)) => {
                                let value =
                                    x.extension.clone().map(Into::into).unwrap_or(FieldValue::Null);
                                (context, value)
                            }
                            _ => unreachable!(),
                        }
                    })),
                    "__typename" => Box::new(contexts.map(|context| {
                        match context.active_vertex::<Self::Vertex>() {
                            None => (context, FieldValue::Null),
                            Some(_) => (context, "File".into()),
                        }
                    })),
                    _ => todo!(),
                }
            }
            _ => todo!(),
        }
    }

    fn resolve_neighbors<V: AsVertex<Self::Vertex> + 'a>(
        &self,
        contexts: ContextIterator<'a, V>,
        type_name: &Arc<str>,
        edge_name: &Arc<str>,
        parameters: &EdgeParameters,
        resolve_info: &ResolveEdgeInfo,
    ) -> ContextOutcomeIterator<'a, V, VertexIterator<'a, Self::Vertex>> {
        match (type_name.as_ref(), edge_name.as_ref()) {
            ("Directory", "out_Directory_ContainsFile") => {
                let iterator = EdgeResolverIterator::new(
                    self.origin.clone(),
                    contexts,
                    directory_contains_file_handler,
                );
                Box::from(iterator)
            }
            ("Directory", "out_Directory_Subdirectory") => {
                let iterator = EdgeResolverIterator::new(
                    self.origin.clone(),
                    contexts,
                    directory_subdirectory_handler,
                );
                Box::from(iterator)
            }
            _ => unimplemented!(),
        }
    }

    fn resolve_coercion<V: AsVertex<Self::Vertex> + 'a>(
        &self,
        contexts: ContextIterator<'a, V>,
        type_name: &Arc<str>,
        coerce_to_type: &Arc<str>,
        resolve_info: &ResolveInfo,
    ) -> ContextOutcomeIterator<'a, V, bool> {
        todo!()
    }
}
