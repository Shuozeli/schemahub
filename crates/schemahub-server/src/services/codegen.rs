use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    GetDescriptorsRequest, GetDescriptorsResponse, PreviewCodegenRequest, PreviewCodegenResponse,
    codegen_service_server::CodegenService,
};

pub struct CodegenServiceImpl {
    #[allow(dead_code)]
    core: Arc<Core>,
}

impl CodegenServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl CodegenService for CodegenServiceImpl {
    async fn get_descriptors(
        &self,
        _request: Request<GetDescriptorsRequest>,
    ) -> Result<Response<GetDescriptorsResponse>, Status> {
        Err(Status::unimplemented("GetDescriptors not yet implemented"))
    }

    async fn preview_codegen(
        &self,
        _request: Request<PreviewCodegenRequest>,
    ) -> Result<Response<PreviewCodegenResponse>, Status> {
        Err(Status::unimplemented("PreviewCodegen not yet implemented"))
    }
}
