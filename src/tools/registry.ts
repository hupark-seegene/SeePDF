/**
 * Registers every tool module on the controller. Imported **dynamically** by the annotation host
 * so that none of the tool code reaches the critical-path bundle (STAGE1D_NOTES §6).
 */
import { toolController } from "./ToolController";
import { makeMarkupTool } from "./markup";
import { inkTool } from "./ink";
import { makeShapeTool } from "./shapes";
import { makeLineTool } from "./line";
import { noteTool } from "./note";
import { textBoxTool } from "./textbox";
import { makeStampTool } from "./stamp";
import { eraserTool } from "./eraser";
import { selectTool } from "./select";
import { snapshotTool } from "./snapshot";

let registered = false;

export function registerTools(): void {
  if (registered) return;
  registered = true;
  toolController.register(selectTool);
  toolController.register(makeMarkupTool("highlight"));
  toolController.register(makeMarkupTool("underline"));
  toolController.register(makeMarkupTool("strikeout"));
  toolController.register(makeMarkupTool("squiggly"));
  toolController.register(noteTool);
  toolController.register(inkTool);
  toolController.register(eraserTool);
  toolController.register(makeShapeTool("rectangle"));
  toolController.register(makeShapeTool("ellipse"));
  toolController.register(makeLineTool("line"));
  toolController.register(makeLineTool("arrow"));
  toolController.register(textBoxTool);
  toolController.register(makeStampTool("stamp"));
  toolController.register(makeStampTool("signature"));
  // v0.3 (V1): the marquee is the viewer's; the controller knows its cursor and Esc
  toolController.register(snapshotTool);
}

/** Test seam: lets a suite re-register after `toolController` was driven by another test. */
export function resetToolRegistry(): void {
  registered = false;
}
