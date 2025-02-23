// Copyright (C) 2021 Bosutech XXI S.L.
//
// nucliadb is offered under the AGPL v3.0 and as commercial software.
// For commercial licensing, contact us at info@nuclia.com.
//
// AGPL:
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.
//

use std::collections::HashSet;
use std::fmt::Debug;
use std::fs;
use std::time::SystemTime;

use nucliadb_protos::{
    OrderBy, ParagraphSearchRequest, ParagraphSearchResponse, ResourceId, SuggestRequest,
};
use nucliadb_service_interface::prelude::*;
use search_query::{search_query, suggest_query};
use tantivy::collector::{Count, DocSetCollector, FacetCollector, MultiCollector, TopDocs};
use tantivy::query::{AllQuery, Query, QueryParser, TermQuery};
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexSettings, IndexSortByField, Order, ReloadPolicy};
use tracing::*;

use super::schema::ParagraphSchema;
use crate::search_query;
use crate::search_query::SharedTermC;
use crate::search_response::{SearchBm25Response, SearchFacetsResponse, SearchIntResponse};

const FUZZY_DISTANCE: usize = 1;

pub struct ParagraphReaderService {
    index: Index,
    pub schema: ParagraphSchema,
    pub reader: IndexReader,
}

impl Debug for ParagraphReaderService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextService")
            .field("index", &self.index)
            .field("schema", &self.schema)
            .finish()
    }
}

impl ParagraphReader for ParagraphReaderService {
    #[tracing::instrument(skip_all)]
    fn count(&self) -> InternalResult<usize> {
        let id: Option<String> = None;
        let time = SystemTime::now();
        let searcher = self.reader.searcher();
        let count = searcher.search(&AllQuery, &Count).unwrap_or_default();
        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Ending at: {v} ms");
        }
        Ok(count)
    }
    #[tracing::instrument(skip_all)]
    fn suggest(&self, request: &SuggestRequest) -> InternalResult<Self::Response> {
        let id = Some(&request.shard);
        let time = SystemTime::now();

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Creating query: starts at {v} ms");
        }
        let parser = QueryParser::for_index(&self.index, vec![self.schema.text]);
        let no_results = 10;
        let text = self.adapt_text(&parser, &request.body);
        let (original, termc, fuzzied) =
            suggest_query(&parser, &text, request, &self.schema, FUZZY_DISTANCE as u8);
        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Creating query: ends at {v} ms");
        }

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Searching: starts at {v} ms");
        }
        let searcher = self.reader.searcher();
        let topdocs = TopDocs::with_limit(no_results);
        let mut results = searcher.search(&original, &topdocs).unwrap();
        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Searching: ends at {v} ms");
        }

        if results.is_empty() {
            if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
                info!("{id:?} - Trying fuzzy: starts at {v} ms");
            }
            let topdocs = TopDocs::with_limit(no_results - results.len());
            match searcher.search(&fuzzied, &topdocs) {
                Ok(mut fuzzied) => results.append(&mut fuzzied),
                Err(err) => error!("{err:?} during suggest"),
            }
            if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
                info!("{id:?} - Trying fuzzy: ends at {v} ms");
            }
        }

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Ending at: {v} ms");
        }
        Ok(ParagraphSearchResponse::from(SearchBm25Response {
            facets_count: None,
            facets: vec![],
            top_docs: results,
            termc: termc.get_termc(),
            text_service: self,
            query: &text,
            page_number: 1,
            results_per_page: 10,
        }))
    }
}

impl ReaderChild for ParagraphReaderService {
    type Request = ParagraphSearchRequest;
    type Response = ParagraphSearchResponse;
    fn stop(&self) -> InternalResult<()> {
        info!("Stopping Paragraph Reader Service");
        Ok(())
    }
    #[tracing::instrument(skip_all)]
    fn search(&self, request: &Self::Request) -> InternalResult<Self::Response> {
        let id = Some(&request.id);
        let time = SystemTime::now();

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Creating query: starts at {v} ms");
        }
        let parser = QueryParser::for_index(&self.index, vec![self.schema.text]);
        let results = request.result_per_page as usize;
        let offset = results * request.page_number as usize;
        let order_field = self.get_order_field(&request.order);
        let facets: Vec<_> = request
            .faceted
            .as_ref()
            .map(|v| {
                v.tags
                    .iter()
                    .filter(|s| ParagraphReaderService::is_valid_facet(s))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let text = self.adapt_text(&parser, &request.body);
        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Creating query: ends at {v} ms");
        }

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Searching: starts at {v} ms");
        }
        let (original, termc, fuzzied) =
            search_query(&parser, &text, request, &self.schema, FUZZY_DISTANCE as u8);
        let mut searcher = Searcher {
            request,
            results,
            offset,
            facets: &facets,
            order_field,
            text: &text,
            only_faceted: request.only_faceted,
        };
        let mut response = searcher.do_search(termc.clone(), original, self);
        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Searching: ends at {v} ms");
        }

        if response.results.is_empty() {
            if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
                info!("{id:?} - Applying fuzzy: starts at {v} ms");
            }
            searcher.results -= response.results.len();
            let fuzzied = searcher.do_search(termc, fuzzied, self);
            let filter = response
                .results
                .iter()
                .map(|r| r.paragraph.clone())
                .collect::<HashSet<_>>();
            fuzzied
                .results
                .into_iter()
                .filter(|r| !filter.contains(&r.paragraph))
                .for_each(|r| response.results.push(r));
            response.total = response.results.len() as i32;
            response.fuzzy_distance = FUZZY_DISTANCE as i32;
            if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
                info!("{id:?} - Applying fuzzy: ends at {v} ms");
            }
        }

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Producing results: starts at {v} ms");
        }
        let total = response.results.len() as f32;
        response.results.iter_mut().enumerate().for_each(|(i, r)| {
            if let Some(sc) = &mut r.score {
                sc.booster = total - (i as f32);
            }
        });
        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Producing results: starts at {v} ms");
        }

        if let Ok(v) = time.elapsed().map(|s| s.as_millis()) {
            info!("{id:?} - Ending at: {v} ms");
        }
        Ok(response)
    }
    #[tracing::instrument(skip_all)]
    fn reload(&self) {
        self.reader.reload().unwrap();
    }
    #[tracing::instrument(skip_all)]
    fn stored_ids(&self) -> Vec<String> {
        self.keys()
    }
}

impl ParagraphReaderService {
    pub fn find_one(&self, resource_id: &ResourceId) -> tantivy::Result<Option<Document>> {
        let uuid_field = self.schema.uuid;
        let uuid_term = Term::from_field_text(uuid_field, &resource_id.uuid);
        let uuid_query = TermQuery::new(uuid_term, IndexRecordOption::Basic);

        let searcher = self.reader.searcher();

        let top_docs = searcher.search(&uuid_query, &TopDocs::with_limit(1))?;

        top_docs
            .first()
            .map(|(_, doc_address)| searcher.doc(*doc_address))
            .transpose()
    }

    pub fn find_resource(&self, resource_id: &ResourceId) -> tantivy::Result<Vec<Document>> {
        let uuid_field = self.schema.uuid;
        let uuid_term = Term::from_field_text(uuid_field, &resource_id.uuid);
        let uuid_query = TermQuery::new(uuid_term, IndexRecordOption::Basic);

        let searcher = self.reader.searcher();

        let top_docs = searcher.search(&uuid_query, &TopDocs::with_limit(1000))?;
        let mut docs = Vec::with_capacity(1000);

        for (_score, doc_address) in top_docs {
            let doc = searcher.doc(doc_address)?;
            docs.push(doc);
        }

        Ok(docs)
    }
    #[tracing::instrument(skip_all)]
    pub fn start(config: &ParagraphConfig) -> InternalResult<Self> {
        ParagraphReaderService::open(config).or_else(|_| ParagraphReaderService::new(config))
    }
    #[tracing::instrument(skip_all)]
    pub fn new(config: &ParagraphConfig) -> InternalResult<ParagraphReaderService> {
        match ParagraphReaderService::new_inner(config) {
            Ok(service) => Ok(service),
            Err(e) => Err(Box::new(ParagraphError { msg: e.to_string() })),
        }
    }
    #[tracing::instrument(skip_all)]
    pub fn open(config: &ParagraphConfig) -> InternalResult<ParagraphReaderService> {
        match ParagraphReaderService::open_inner(config) {
            Ok(service) => Ok(service),
            Err(e) => Err(Box::new(ParagraphError { msg: e.to_string() })),
        }
    }

    pub fn new_inner(config: &ParagraphConfig) -> tantivy::Result<ParagraphReaderService> {
        let paragraph_schema = ParagraphSchema::default();
        fs::create_dir_all(&config.path)?;
        let mut index_builder = Index::builder().schema(paragraph_schema.schema.clone());
        let settings = IndexSettings {
            sort_by_field: Some(IndexSortByField {
                field: "created".to_string(),
                order: Order::Desc,
            }),
            ..Default::default()
        };

        index_builder = index_builder.settings(settings);
        let index = index_builder.create_in_dir(&config.path).unwrap();
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommit)
            .try_into()?;
        Ok(ParagraphReaderService {
            index,
            reader,
            schema: paragraph_schema,
        })
    }

    pub fn open_inner(config: &ParagraphConfig) -> tantivy::Result<ParagraphReaderService> {
        let paragraph_schema = ParagraphSchema::default();
        let index = Index::open_in_dir(&config.path)?;

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommit)
            .try_into()?;

        Ok(ParagraphReaderService {
            index,
            reader,
            schema: paragraph_schema,
        })
    }

    fn adapt_text(&self, parser: &QueryParser, text: &str) -> String {
        match text.trim() {
            "" => text.to_string(),
            text => parser
                .parse_query(text)
                .map(|_| text.to_string())
                .unwrap_or_else(|_| format!("\"{}\"", text.replace('"', ""))),
        }
    }

    fn keys(&self) -> Vec<String> {
        let searcher = self.reader.searcher();
        searcher
            .search(&AllQuery, &DocSetCollector)
            .unwrap()
            .into_iter()
            .map(|addr| {
                searcher
                    .doc(addr)
                    .unwrap()
                    .get_first(self.schema.uuid)
                    .expect("document doesn't appear to have uuid.")
                    .as_text()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }
    fn is_valid_facet(maybe_facet: &str) -> bool {
        Facet::from_text(maybe_facet).is_ok()
    }

    fn get_order_field(&self, order: &Option<OrderBy>) -> Option<Field> {
        match order.as_ref().map(|o| o.field.as_str()) {
            Some("created") => Some(self.schema.created),
            Some("modified") => Some(self.schema.modified),
            _ => None,
        }
    }
}

struct Searcher<'a> {
    request: &'a ParagraphSearchRequest,
    results: usize,
    offset: usize,
    facets: &'a [String],
    order_field: Option<Field>,
    text: &'a str,
    only_faceted: bool,
}
impl<'a> Searcher<'a> {
    fn do_search(
        &self,
        termc: SharedTermC,
        query: Box<dyn Query>,
        service: &ParagraphReaderService,
    ) -> ParagraphSearchResponse {
        let searcher = service.reader.searcher();
        let facet_collector = self.facets.iter().fold(
            FacetCollector::for_field(service.schema.facets),
            |mut collector, facet| {
                collector.add_facet(Facet::from(facet));
                collector
            },
        );
        if self.only_faceted {
            // No query search, just facets
            let facets_count = searcher.search(&query, &facet_collector).unwrap();
            ParagraphSearchResponse::from(SearchFacetsResponse {
                text_service: service,
                facets_count: Some(facets_count),
                facets: self.facets.to_vec(),
            })
        } else if self.facets.is_empty() {
            // Only query no facets
            let extra_result = self.results + 1;
            match self.order_field {
                Some(order_field) => {
                    let topdocs = TopDocs::with_limit(extra_result)
                        .and_offset(self.offset)
                        .order_by_u64_field(order_field);
                    let top_docs = searcher.search(&query, &topdocs).unwrap();
                    ParagraphSearchResponse::from(SearchIntResponse {
                        facets_count: None,
                        facets: self.facets.to_vec(),
                        top_docs,
                        termc: termc.get_termc(),
                        text_service: service,
                        query: self.text,
                        page_number: self.request.page_number,
                        results_per_page: self.results as i32,
                    })
                }
                None => {
                    let topdocs = TopDocs::with_limit(extra_result).and_offset(self.offset);
                    let top_docs = searcher.search(&query, &topdocs).unwrap();
                    ParagraphSearchResponse::from(SearchBm25Response {
                        facets_count: None,
                        facets: self.facets.to_vec(),
                        top_docs,
                        termc: termc.get_termc(),
                        text_service: service,
                        query: self.text,
                        page_number: self.request.page_number,
                        results_per_page: self.results as i32,
                    })
                }
            }
        } else {
            let extra_result = self.results + 1;

            match self.order_field {
                Some(order_field) => {
                    let topdocs = TopDocs::with_limit(extra_result)
                        .and_offset(self.offset)
                        .order_by_u64_field(order_field);
                    let mut multicollector = MultiCollector::new();
                    let facet_handler = multicollector.add_collector(facet_collector);
                    let topdocs_handler = multicollector.add_collector(topdocs);
                    let mut multi_fruit = searcher.search(&query, &multicollector).unwrap();
                    let facets_count = facet_handler.extract(&mut multi_fruit);
                    let top_docs = topdocs_handler.extract(&mut multi_fruit);
                    ParagraphSearchResponse::from(SearchIntResponse {
                        facets_count: Some(facets_count),
                        facets: self.facets.to_vec(),
                        top_docs,
                        termc: termc.get_termc(),
                        text_service: service,
                        query: self.text,
                        page_number: self.request.page_number,
                        results_per_page: self.results as i32,
                    })
                }
                None => {
                    let topdocs = TopDocs::with_limit(extra_result).and_offset(self.offset);
                    let mut multicollector = MultiCollector::new();
                    let facet_handler = multicollector.add_collector(facet_collector);
                    let topdocs_handler = multicollector.add_collector(topdocs);
                    let mut multi_fruit = searcher.search(&query, &multicollector).unwrap();
                    let facets_count = facet_handler.extract(&mut multi_fruit);
                    let top_docs = topdocs_handler.extract(&mut multi_fruit);
                    ParagraphSearchResponse::from(SearchBm25Response {
                        facets_count: Some(facets_count),
                        facets: self.facets.to_vec(),
                        top_docs,
                        termc: termc.get_termc(),
                        text_service: service,
                        query: self.text,
                        page_number: self.request.page_number,
                        results_per_page: self.results as i32,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;
    use std::time::SystemTime;

    use nucliadb_protos::{
        Faceted, Filter, IndexParagraph, IndexParagraphs, OrderBy, Resource, ResourceId, Timestamps,
    };
    use prost_types::Timestamp;
    use tantivy::collector::Count;
    use tantivy::query::AllQuery;
    use tempdir::TempDir;

    use super::*;
    use crate::writer::ParagraphWriterService;

    fn create_resource(shard_id: String) -> Resource {
        const UUID: &str = "f56c58ac-b4f9-4d61-a077-ffccaadd0001";
        let resource_id = ResourceId {
            shard_id: shard_id.to_string(),
            uuid: UUID.to_string(),
        };

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let timestamp = Timestamp {
            seconds: now.as_secs() as i64,
            nanos: 0,
        };

        let metadata = nucliadb_protos::IndexMetadata {
            created: Some(timestamp.clone()),
            modified: Some(timestamp),
        };

        const DOC1_TI: &str = "This is the first document";
        const DOC1_P1: &str = "This is the text of the second paragraph.";
        const DOC1_P2: &str = "This should be enough to test the tantivy.";
        const DOC1_P3: &str = "But I wanted to make it three anyway.";

        let ti_title = nucliadb_protos::TextInformation {
            text: DOC1_TI.to_string(),
            labels: vec!["/e/mylabel".to_string()],
        };

        let ti_body = nucliadb_protos::TextInformation {
            text: DOC1_P1.to_string() + DOC1_P2 + DOC1_P3,
            labels: vec!["/f/body".to_string(), "/l/mylabel2".to_string()],
        };

        let mut texts = HashMap::new();
        texts.insert("title".to_string(), ti_title);
        texts.insert("body".to_string(), ti_body);

        let p1 = IndexParagraph {
            start: 0,
            end: DOC1_P1.len() as i32,
            sentences: HashMap::new(),
            field: "body".to_string(),
            labels: vec!["/e/myentity".to_string()],
            index: 0,
            split: "".to_string(),
            repeated_in_field: false,
            metadata: None,
        };
        let p1_uuid = format!("{}/{}/{}-{}", UUID, "body", 0, DOC1_P1.len());

        let p2 = IndexParagraph {
            start: DOC1_P1.len() as i32,
            end: (DOC1_P1.len() + DOC1_P2.len()) as i32,
            sentences: HashMap::new(),
            field: "body".to_string(),
            labels: vec![
                "/tantivy".to_string(),
                "/test".to_string(),
                "/label1".to_string(),
            ],
            index: 1,
            split: "".to_string(),
            repeated_in_field: false,
            metadata: None,
        };
        let p2_uuid = format!(
            "{}/{}/{}-{}",
            UUID,
            "body",
            DOC1_P1.len(),
            DOC1_P1.len() + DOC1_P2.len()
        );

        let p3 = IndexParagraph {
            start: (DOC1_P1.len() + DOC1_P2.len()) as i32,
            end: (DOC1_P1.len() + DOC1_P2.len() + DOC1_P3.len()) as i32,
            sentences: HashMap::new(),
            field: "body".to_string(),
            labels: vec!["/three".to_string(), "/label2".to_string()],
            index: 2,
            split: "".to_string(),
            repeated_in_field: false,
            metadata: None,
        };
        let p3_uuid = format!(
            "{}/{}/{}-{}",
            UUID,
            "body",
            DOC1_P1.len() + DOC1_P2.len(),
            DOC1_P1.len() + DOC1_P2.len() + DOC1_P3.len()
        );

        let body_paragraphs = IndexParagraphs {
            paragraphs: [(p1_uuid, p1), (p2_uuid, p2), (p3_uuid, p3)]
                .into_iter()
                .collect(),
        };

        let p4 = IndexParagraph {
            start: 0,
            end: DOC1_TI.len() as i32,
            sentences: HashMap::new(),
            field: "title".to_string(),
            labels: vec!["/c/ool".to_string()],
            index: 3,
            split: "".to_string(),
            repeated_in_field: false,
            metadata: None,
        };
        let p4_uuid = format!("{}/{}/{}-{}", UUID, "body", 0, DOC1_TI.len());

        let title_paragraphs = IndexParagraphs {
            paragraphs: [(p4_uuid, p4)].into_iter().collect(),
        };

        let paragraphs = [
            ("body".to_string(), body_paragraphs),
            ("title".to_string(), title_paragraphs),
        ]
        .into_iter()
        .collect();

        Resource {
            resource: Some(resource_id),
            metadata: Some(metadata),
            texts,
            status: nucliadb_protos::resource::ResourceStatus::Processed as i32,
            labels: vec!["/l/mylabel_resource".to_string()],
            paragraphs,
            paragraphs_to_delete: vec![],
            sentences_to_delete: vec![],
            relations_to_delete: vec![],
            relations: vec![],
            vectors: HashMap::default(),
            vectors_to_delete: HashMap::default(),
            shard_id,
        }
    }

    #[test]
    fn test_new_paragraph() -> anyhow::Result<()> {
        let dir = TempDir::new("payload_dir").unwrap();
        let psc = ParagraphConfig {
            path: dir.path().as_os_str().to_os_string().into_string().unwrap(),
        };
        let mut paragraph_writer_service = ParagraphWriterService::start(&psc).unwrap();
        let resource1 = create_resource("shard1".to_string());
        let _ = paragraph_writer_service.set_resource(&resource1);

        let paragraph_reader_service = ParagraphReaderService::start(&psc).unwrap();

        let reader = paragraph_writer_service.index.reader()?;
        let searcher = reader.searcher();

        let (_top_docs, count) = searcher.search(&AllQuery, &(TopDocs::with_limit(10), Count))?;
        assert_eq!(count, 4);

        const UUID: &str = "f56c58ac-b4f9-4d61-a077-ffccaadd0001";
        let rid = ResourceId {
            shard_id: "shard1".to_string(),
            uuid: UUID.to_string(),
        };

        let result = paragraph_reader_service.find_resource(&rid).unwrap();
        assert!(!result.is_empty());
        let result = paragraph_reader_service.find_one(&rid).unwrap();
        assert!(result.is_some());

        // Testing filtering one filter from resource, one from field and one from paragraph

        let filter = Filter {
            tags: vec![
                "/l/mylabel_resource".to_string(),
                "/c/ool".to_string(),
                "/e/mylabel".to_string(),
            ],
        };

        let faceted = Faceted {
            tags: vec![
                "".to_string(),
                "/l".to_string(),
                "/e".to_string(),
                "/c".to_string(),
            ],
        };

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

        let timestamp = Timestamp {
            seconds: now.as_secs() as i64,
            nanos: 0,
        };

        let old_timestamp = Timestamp {
            seconds: 0_i64,
            nanos: 0,
        };

        let timestamps = Timestamps {
            from_modified: Some(old_timestamp.clone()),
            to_modified: Some(timestamp.clone()),
            from_created: Some(old_timestamp),
            to_created: Some(timestamp),
        };

        let order = OrderBy {
            field: "created".to_string(),
            r#type: 0,
        };

        // Search on all paragraphs faceted
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: UUID.to_string(),
            body: "".to_string(),
            fields: vec!["body".to_string(), "title".to_string()],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 0);

        // Search on all paragraphs
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: UUID.to_string(),
            body: "".to_string(),
            fields: vec![],
            filter: Some(filter.clone()),
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 1);

        // Search on all paragraphs without fields
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: UUID.to_string(),
            body: "".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 4);

        // Search on all paragraphs in resource with typo
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: UUID.to_string(),
            body: "shoupd enaugh".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 1);

        // Search on all paragraphs in resource with typo
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: UUID.to_string(),
            body: "\"should\" enaugh".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 1);

        // Search typo on all paragraph
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "shoupd enaugh".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 1);

        // Search with invalid and unbalanced grammar
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "shoupd + enaugh\"".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.query, "\"shoupd + enaugh\"");
        assert_eq!(result.total, 0);

        // Search with invalid grammar
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "shoupd + enaugh".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.query, "\"shoupd + enaugh\"");
        assert_eq!(result.total, 0);

        // Empty search
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: true,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 4);

        // Search filter all paragraphs
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "this is the".to_string(),
            fields: vec![],
            filter: None,
            faceted: Some(faceted.clone()),
            order: Some(order),
            page_number: 0,
            result_per_page: 20,
            timestamps: Some(timestamps.clone()),
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 3);
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "this is the".to_string(),
            fields: vec![],
            filter: Some(filter),
            faceted: Some(faceted),
            order: None, // Some(order),
            page_number: 0,
            result_per_page: 20,
            timestamps: Some(timestamps),
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 1);

        // Search typo on all paragraph
        let search = ParagraphSearchRequest {
            id: "shard1".to_string(),
            uuid: "".to_string(),
            body: "\"shoupd\"".to_string(),
            fields: vec![],
            filter: None,
            faceted: None,
            order: None,
            page_number: 0,
            result_per_page: 20,
            timestamps: None,
            reload: false,
            with_duplicates: false,
            only_faceted: false,
        };
        let result = paragraph_reader_service.search(&search).unwrap();
        assert_eq!(result.total, 0);
        Ok(())
    }
}
