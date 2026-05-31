/** A value that may be returned synchronously or asynchronously. */
export type Awaitable<T> = T | Promise<T>;

/** A stage in a `pipeline` call. */
export type PipelineStage<Previous = unknown, Item = unknown, Result = unknown> = (
  previous: Previous,
  item: Item,
  index: number,
) => Awaitable<Result>;

/**
 * Runs items through sequential stages without a barrier between stages.
 *
 * Each item advances to its next stage as soon as that item is ready. If a stage
 * throws for an item, that item resolves to `null` and remaining stages are skipped.
 */
export type PipelineFn = {
  <Item>(items: readonly Item[]): Promise<Item[]>;
  <Item, Stage1>(
    items: readonly Item[],
    stage1: PipelineStage<Item, Item, Stage1>,
  ): Promise<Array<Awaited<Stage1> | null>>;
  <Item, Stage1, Stage2>(
    items: readonly Item[],
    stage1: PipelineStage<Item, Item, Stage1>,
    stage2: PipelineStage<Awaited<Stage1>, Item, Stage2>,
  ): Promise<Array<Awaited<Stage2> | null>>;
  <Item, Stage1, Stage2, Stage3>(
    items: readonly Item[],
    stage1: PipelineStage<Item, Item, Stage1>,
    stage2: PipelineStage<Awaited<Stage1>, Item, Stage2>,
    stage3: PipelineStage<Awaited<Stage2>, Item, Stage3>,
  ): Promise<Array<Awaited<Stage3> | null>>;
  <Item, Stage1, Stage2, Stage3, Stage4>(
    items: readonly Item[],
    stage1: PipelineStage<Item, Item, Stage1>,
    stage2: PipelineStage<Awaited<Stage1>, Item, Stage2>,
    stage3: PipelineStage<Awaited<Stage2>, Item, Stage3>,
    stage4: PipelineStage<Awaited<Stage3>, Item, Stage4>,
  ): Promise<Array<Awaited<Stage4> | null>>;
  <Item, Stage1, Stage2, Stage3, Stage4, Stage5>(
    items: readonly Item[],
    stage1: PipelineStage<Item, Item, Stage1>,
    stage2: PipelineStage<Awaited<Stage1>, Item, Stage2>,
    stage3: PipelineStage<Awaited<Stage2>, Item, Stage3>,
    stage4: PipelineStage<Awaited<Stage3>, Item, Stage4>,
    stage5: PipelineStage<Awaited<Stage4>, Item, Stage5>,
  ): Promise<Array<Awaited<Stage5> | null>>;
  <Item, Stage1, Stage2, Stage3, Stage4, Stage5, Stage6>(
    items: readonly Item[],
    stage1: PipelineStage<Item, Item, Stage1>,
    stage2: PipelineStage<Awaited<Stage1>, Item, Stage2>,
    stage3: PipelineStage<Awaited<Stage2>, Item, Stage3>,
    stage4: PipelineStage<Awaited<Stage3>, Item, Stage4>,
    stage5: PipelineStage<Awaited<Stage4>, Item, Stage5>,
    stage6: PipelineStage<Awaited<Stage5>, Item, Stage6>,
  ): Promise<Array<Awaited<Stage6> | null>>;
  <Item, Result = unknown>(
    items: readonly Item[],
    ...stages: readonly PipelineStage<unknown, Item, unknown>[]
  ): Promise<Array<Result | null>>;
};
